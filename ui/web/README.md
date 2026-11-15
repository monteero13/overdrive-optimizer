# Overdrive 1.0 — UI (Fase 2: prototipo con motor simulado)

Frontend "Liquid Telemetry" de Overdrive 1.0. En esta fase funciona contra un **motor simulado** (`src/mock/`) que habla exactamente el protocolo `overdrive.v1` ([docs/architecture/ipc-protocol.md](../../docs/architecture/ipc-protocol.md)). En la Fase 3 se sustituye `MockTransport` por un transporte de Tauri que habla con el servicio real a través del named pipe. La UI no cambia.

## Uso

```bash
cd ui/web
npm install
npm run dev        # http://localhost:5173
npm test           # tests del protocolo, del motor simulado y de i18n (Node ≥ 22)
npm run build      # typecheck + build de producción
```

### Panel "Motor simulado" (abajo a la izquierda)

Lleva la interfaz a cualquier estado, incluidos los caminos malos:

| Control | Qué demuestra |
|---|---|
| Abrir / cerrar juego | Sesión en vivo, HUD, reversión al cerrar, informe (capa 3), pregunta "¿Mantenerlo siempre?" |
| Operaciones lentas | La operación supera su plazo → `timed_out` → se revierte y se informa |
| Fallar la próxima operación | Un paso falla → se revierte lo aplicado → mensaje accionable |
| Simular caída del servicio | El enlace se cae, las operaciones desaparecen (sin overlays atascados), y al reconectar aparece "Tu PC está como estaba" |
| Perder un evento | Hueco de `seq` → el cliente pide un snapshot completo (contador de *resyncs*) |
| Windows Update / driver | Drift Guardian repara un cambio persistente |
| Temperatura alta / batería | Estados del Pulse y consejo estructural |
| PC simulado | Tres máquinas: portátil con 2 gráficas (Tier 3), sobremesa de entrada (Tier 2) y APU (Tier 1) |
| Ver bandeja | Réplica del menú del tray (Reanudar todo / Deshacer todo) |

### Enlaces directos (solo en desarrollo)

`?preset=desktop_entry&mode=expert&theme=light&locale=en&view=pc&plan=1&session=1&report=1&coach=1&dev=0`

## Estructura

```
src/proto/types.ts     tipos del protocolo (se generarán desde crates/overdrive-proto en la Fase 3)
src/engine/            EngineClient (snapshot + merge patches + resync), transporte, merge patch RFC 7396
src/mock/              MockService (servicio simulado), catálogo de muestra, presets de hardware
src/i18n/              es (principal) + en, con paridad comprobada por test
src/ui/                app (orquestación), views (capas 1 y 2, Mi PC, historial), report (capa 3),
                       overlays (operaciones, preguntas, HUD, coach, ajustes, tray, panel dev), pulse, magnetic
```

## Criterio de terminado de la Fase 2

- [x] Capas 1, 2 y 3, Pulse, consejos estructurales, estados del tray, modos Fácil y Experto, tema oscuro y claro, ES y EN
- [x] Motor simulado que habla el protocolo real, con escenarios de fallo, lentitud, cancelación, caída y hueco de `seq`
- [x] Todo estado de la UI es alcanzable desde el estado del motor simulado (panel y enlaces directos)
- [x] Sin overlays atascados: los modales de progreso solo existen mientras la operación está en el snapshot. Lo verifican los tests de fallo, timeout, cancelación y caída
- [x] Animaciones reducidas: respeta `prefers-reduced-motion` y el ajuste propio; el Pulse se dibuja estático
- [x] Ligero en iGPU: Pulse en Canvas 2D a 30 fps como máximo, DPR limitado a 1,5, **sin bucle de animación mientras hay un juego abierto**, ningún framework
- [x] Comprobado a 1366×768 con capturas headless en ambos temas
- [x] Gráficos: paleta validada (contraste y daltonismo) en claro y oscuro, leyenda y etiquetas directas, cursor con tooltip, navegación con teclado y vista en tabla
