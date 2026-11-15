# Migración desde la v0.2.1

## 1. Módulos

| v0.2.1 | Destino en 1.0 | Qué se conserva | Qué cambia |
|---|---|---|---|
| `src/main/ps.js` | — (eliminado) | — | Sin PowerShell en el servicio; puertos Win32 en `overdrive-win` |
| `src/main/system.js` | **A · Profiler** (`HardwareSource` en `overdrive-win`) | Datos recogidos, URLs oficiales de drivers por fabricante, criterio de "driver muy desactualizado" | WMI/DXGI/SetupAPI nativos, `Confidence` por campo, tiers, problemas estructurales, BIOS Coach |
| `src/main/monitor.js` | **B · Telemetry** | Idea del monitor en vivo | `nvidia-smi` + PowerShell sustituidos por PDH + DXGI `QueryVideoMemoryInfo` + PresentMon (frametimes, GPU Busy) |
| `src/main/benchmark.js` | **E · benchmark guiado A/B** | Concepto de benchmark | Se pasa de benchmark sintético de CPU/disco a **medir el juego real** (1 % low, frametime) con protocolo estadístico |
| `src/main/tweaks.js` | **C · Catalog** (`catalog/*.toml`) + primitivas | Conocimiento de qué claves tocar | Declarativo, con captura del estado original; ver auditoría §2 |
| `src/main/games.js` | **E · `GameDetector`** | Parser de bibliotecas de Steam (VDF), lista `KNOWN_BACKGROUND` como semilla de categorías | + Epic, EA, Ubisoft, Battle.net, GOG, Xbox; detección por ETW en vez de sondeo cada 6 s |
| `src/main/suspend.js` | **F · Safety Policy** (`background_action`) | Lista `NO_DEPRIO`, la idea de restaurar la prioridad original por `(pid, start)` y el rescate de procesos congelados | Política completa de *nunca tocar*, EcoQoS por defecto, journal en `background_actions`. El `EmptyWorkingSet` puntual pasa a tweak `DISPUTED` medido (es primo de los "RAM boosters") |
| `src/main/profiles.js` | **E · perfiles por juego** + **H · Ayuda a un amigo** | Formato de código `OVD-…` como inspiración | El perfil deja de ser una lista fija de tweaks: es el resultado aprendido por A/B. Los códigos de compartir van firmados y redactados |
| `src/main/updater.js` | Actualizador de Tauri 2 | Canal GitHub Releases | Manifiestos firmados, despliegue escalonado |
| `src/main/main.js` (IPC, vigilante) | `overdrive-svc` + `ipc-protocol.md` | Lista de capacidades expuestas | Métodos cerrados, operaciones con `opId`/deadline, snapshot de estado |
| `src/renderer/*` | `ui/web` (Fase 2) | Modos Fácil/Experto, textos en español, aprendizaje de UX | Nueva UI "Liquid Telemetry" que pinta `EngineSnapshot` |

**Datos del usuario que importa el instalador 1.0:** `profiles.json` (juego, ejecutable, procesos de fondo, `autoApply`) y `background-state.json` (se usa para restaurar prioridades pendientes antes de desinstalar la v0.2.x).

## 2. Auditoría de los 24 tweaks actuales

Criterios de `goals.md` §5.3: impacto real, evidencia, riesgo, y si captura el estado original.

| Tweak v0.2.1 | Veredicto | Destino / motivo |
|---|---|---|
| `power-high` | **Mantener (rehacer)** | Tweak de **sesión** con el power mode o plan de alto rendimiento, solo con corriente; con batería, avisar. El revert actual vuelve siempre a "Equilibrado" y pierde el plan original |
| `game-mode` | **Mantener** | `PROVEN`, riesgo bajo |
| `gamedvr-off` | **Mantener** | `PROVEN`: la grabación en segundo plano cuesta rendimiento real en equipos modestos |
| `gpu-hags` | **Mantener (condicional)** | `VERSION_DEPENDENT`: solo con la GPU o driver donde está soportado y tras verificarlo con A/B; persistente con reinicio. El revert actual fuerza `1` en vez de restaurar |
| `nvidia-max-perf` | **Rehacer** | Por ejecutable vía NVAPI DRS, no global (global gasta energía y calor siempre) |
| `amd-max-perf` | **Rehacer** | Por ejecutable vía ADLX, donde haya soporte |
| `games-priority` (MMCSS Tasks\Games) | **Sustituir** | Pocos juegos usan MMCSS. La prioridad del juego pasa a `IFEO PerfOptions` |
| `power-throttling-off` | **Experto, `DISPUTED`** | En portátiles puede subir temperaturas y provocar *más* throttling; solo se mantiene si lo demuestra el A/B. El revert actual pone `0` en vez de borrar la clave |
| `visual-fx` | **Mover a "Comodidad", `DISPUTED`** | Mejora la fluidez del escritorio en Tier 1, no los FPS en juego |
| `anim-window`, `anim-taskbar`, `anim-listview`, `anim-dragfull` | **Mover a "Comodidad"** | Igual: solo afectan al escritorio. Opt-in, sin prometer FPS |
| `mouse-accel-off` | **Mover a "Comodidad"** | Preferencia de puntería, no rendimiento. Opt-in |
| `system-responsiveness` | **Experto, `DISPUTED`** | Folklore muy extendido con evidencia débil; solo con verificación |
| `net-throttling-off` | **Eliminar** | Paquete de red de la lista *fuera de alcance* |
| `nagle-off` | **Eliminar** | Ídem; la mayoría de juegos ya desactivan Nagle en sus sockets |
| `qos-bandwidth` | **Eliminar** | El mito del "20 % de ancho de banda reservado": snake oil clásico |
| `tcp-throughput` | **Eliminar** | Paquete "TCP optimizer" |
| `sysmain-off` | **Eliminar del catálogo de rendimiento** | Sin ganancia medible en SSD y perjudicial en HDD; solo se ofrece restaurarlo si otra herramienta lo desactivó |
| `hibernate-off` | **Mover a "Salud del disco"** | No mejora el rendimiento: solo libera espacio y desactiva el inicio rápido. Se ofrece solo si el SSD está casi lleno |
| `clean-temp` | **Mover a "Salud del disco"** | Sin efecto en FPS; solo con poco espacio libre |
| `font-smoothing-off` | **Eliminar** | Empeora la legibilidad sin ningún beneficio en juego |
| `msi-center-install` | **Eliminar como tweak** | Instalar software de terceros no es una optimización; como mucho, un consejo informativo en portátiles MSI |

**Resumen:** de 24 tweaks se mantienen o rehacen 7, 8 pasan a "Comodidad" o modo Experto con verificación obligatoria, 2 pasan a "Salud del disco" y 7 se eliminan.

Los eliminados de la familia de red (`net-throttling-off`, `nagle-off`, `qos-bandwidth`, `tcp-throughput`) pasan a la lista `foreign_harmful` de 1.0, para ofrecer restaurar los valores por defecto. `sysmain-off` también entra en esa lista, pero solo cuando el equipo tiene HDD. `font-smoothing-off` y `msi-center-install` simplemente desaparecen.

> **Recomendación para la v0.2.2 (único cambio en la rama congelada):** sacar de los niveles por defecto (Básica, Media y Agresiva) todos los tweaks marcados como "Eliminar". Así los amigos que ya usan la v0.2.x dejan de aplicárselos.
