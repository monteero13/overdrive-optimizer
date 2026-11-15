# ADR-001 — Stack tecnológico y estrategia de migración

- **Estado:** Propuesto (pendiente de aprobación del autor)
- **Fecha:** 2026-11-15

## Contexto

La v0.2.1 es Electron 33 + Node.js, y hace todo el trabajo de sistema lanzando PowerShell. Funciona, pero choca con los requisitos de `goals.md`:

- **Consumo.** Electron en reposo usa ~150–250 MB en varios procesos. Cada consulta a PowerShell arranca un proceso nuevo (~50–80 MB, 200–600 ms, pico de CPU), y el vigilante de juegos lo hace cada 6 s.
- **Privilegios.** La app entera tiene que correr como administrador para aplicar cambios. No hay un proceso separado con privilegios.
- **Fiabilidad.** Los scripts de PowerShell se escriben como texto, no se pueden testear con fakes y los errores llegan como texto libre.
- **APIs necesarias.** ETW (PresentMon y eventos de proceso), DXGI, SetupAPI, PDH, NVAPI DRS y ADLX son APIs nativas. Desde Node.js necesitan addons nativos o más PowerShell.

## Opciones evaluadas

| Criterio | A. Mantener Electron | B. .NET 8 Native AOT + WinUI 3 | **C. Rust + Tauri 2** |
|---|---|---|---|
| Servicio en reposo (estimado) | n/a (habría que crear servicio aparte) | ~20–35 MB (runtime AOT + GC) | **~3–8 MB** |
| UI abierta | ~200 MB | ~60–120 MB | ~80–150 MB (WebView2) |
| Acceso a Win32 / ETW | Addons nativos (node-gyp) o PowerShell | Muy bueno (P/Invoke, CsWin32); TraceEvent tiene limitaciones con AOT | **Muy bueno**: crate oficial `windows` de Microsoft, `ferrisetw` para ETW |
| Servicio de Windows | Incómodo | Nativo (`BackgroundService`) | Crate `windows-service` |
| UI "Liquid Telemetry" (animaciones, canvas generativo) | Excelente (web) | Posible con Composition/Win2D, más costoso de iterar | **Excelente (web)**: se reutiliza lo aprendido en HTML/CSS/JS |
| Reutilización del código actual | Total | Baja (C#/XAML nuevo) | **Media**: renderer HTML/CSS/JS y conocimiento de tweaks; el motor se reescribe igualmente |
| Seguridad de memoria en el proceso privilegiado | — | Buena | **Excelente** |
| Tamaño del instalador | ~80 MB | ~30–50 MB + Windows App SDK runtime | **~5–15 MB** (WebView2 viene con Win10/11) |
| Curva de aprendizaje para el autor | Ninguna | Media | **Alta en el servicio** (Rust); baja en la UI (web) |

## Decisión

**Opción C: Rust + Tauri 2**, organizada como un workspace de Cargo:

```
Cargo.toml                (workspace, en la raíz junto al package.json actual)
crates/
  overdrive-proto/        tipos IPC compartidos (serde) + generación de tipos TS (specta/ts-rs)
  overdrive-core/         lógica pura: profiler, classifier, catalog, transaction engine,
                          orchestrator, safety, drift, bios coach — SIN Win32
  overdrive-win/          implementaciones Win32 de los puertos de sistema (único lugar con unsafe)
  overdrive-svc/          binario del servicio de Windows
ui/
  src-tauri/              shell Tauri 2 (cliente del pipe, tray, ciclo de vida de la ventana)
  web/                    frontend (TypeScript + Vite; sin framework pesado; animaciones con CSS + Canvas/WebGL)
catalog/                  catálogo declarativo de tweaks (TOML), versionado
games-db/                 base de datos de juegos (anti-cheat, configs) — Fase 7
tools/                    footprint.ps1, fake-game, utilidades de CI
```

### Por qué

1. **Es la única opción que cumple los presupuestos del servicio** (< 15 MB en reposo) con margen.
2. **El proceso con privilegios es la mayor superficie de ataque** (escalada local de privilegios). Rust elimina por diseño las clases de bug de memoria.
3. **La UI sigue siendo web.** Es donde el autor ya es productivo y donde mejor sale la estética Liquid Telemetry. Tauri usa el WebView2 del sistema, así que no empaqueta Chromium.
4. **El crate `windows` lo mantiene Microsoft** y cubre todas las APIs necesarias.

### Qué aceptamos a cambio

- **Curva de Rust.** Se mitiga separando las piezas: la lógica en `overdrive-core` es Rust "fácil" (sin `unsafe` ni Win32) y el código difícil queda aislado en `overdrive-win`, con tests por puerto.
- **WebView2 consume memoria con la ventana abierta.** Se mitiga destruyendo la ventana al minimizar y midiéndolo en CI.
- **Requisitos de desarrollo nuevos:** Rust (rustup, toolchain `stable-x86_64-pc-windows-msvc`) y Visual Studio Build Tools con el workload "Desktop development with C++". Hoy no están instalados en esta máquina.

### Descartadas

- **A (Electron):** no puede cumplir los presupuestos y no resuelve el modelo de privilegios.
- **B (.NET AOT):** alternativa válida y segunda opción. Se descarta porque obliga a abandonar la UI web, el runtime de Windows App SDK añade dependencias y el servicio no baja de ~20 MB en reposo.

## Estrategia de migración

> Nota: `goals.md §9` proponía por defecto una migración incremental (primero el servicio nuevo, con la UI de Electron hablando con él). Tras revisar el código, **recomiendo construir en paralelo** y actualizar `goals.md` en consecuencia.

**Recomendación: v0.2.x congelada + Overdrive 1.0 construido en paralelo en el mismo repositorio.**

| | Incremental (Electron ↔ servicio nuevo) | **Paralelo (recomendado)** |
|---|---|---|
| Trabajo desechable | Alto: hay que adaptar los IPC de Electron a un pipe, y esa UI se tira en la Fase 2 | Bajo |
| Riesgo para los amigos que ya usan la app | Medio: mezcla de dos motores | Bajo: la v0.2.x sigue igual hasta que 1.0 la supere |
| Coherencia | El motor nuevo convive con los `revert` de valores fijos del viejo | Motor nuevo limpio desde el primer día |
| Tiempo hasta valor real | Antes, en teoría | Algo más tarde, pero sin retrabajo |

Plan:

1. **v0.2.x en modo mantenimiento:** solo correcciones críticas. Recomiendo una única v0.2.2 que quite de los niveles por defecto los tweaks clasificados como "eliminar" en [migration.md](migration.md).
2. **1.0 se desarrolla en `crates/` y `ui/`**, siguiendo las fases 2 a 8.
3. **Paso a 1.0:**
   - El instalador de 1.0 detecta la v0.2.x.
   - Ejecuta su reversión: restaura las prioridades guardadas en `background-state.json` y revierte los tweaks que siga detectando.
   - Importa `profiles.json` (juego, ejecutable y procesos de fondo; los ids de tweaks se traducen al catálogo nuevo).
   - Desinstala Electron.
4. **Auto-update:** la última v0.2.x publicada anuncia y descarga el instalador 1.0 desde el mismo canal de GitHub Releases.

## Consecuencias

- `goals.md §9`: fila "Migration" → "Parallel build; v0.2.x frozen; migration via 1.0 installer".
- Antes de empezar la Fase 3 hay que instalar el toolchain de Rust.
- La Fase 2 (prototipo de UI) puede empezar ya con Node.js y Vite, sin Rust, contra el motor simulado que habla el protocolo de [ipc-protocol.md](ipc-protocol.md).
