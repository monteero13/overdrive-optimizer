# Overdrive 1.0 — motor en Rust

| Crate | Qué es |
|---|---|
| `overdrive-core` | Lógica pura, sin Win32: perfilador (módulo A), tier, problemas estructurales, BIOS Coach, telemetría y clasificador (módulo B), motor de transacciones (módulo D) y política de seguridad (módulo F). Testeable con fakes (`testing::FakeSource`, `engine::fake::FakeSystem`) y trazas grabadas. |
| `overdrive-win` | Implementación real para Windows 10/11. Es el único crate con `unsafe` (`src/sys.rs`, `src/telemetry.rs`, `src/system_ops.rs`). Incluye el binario `overdrive-probe`. |

```bash
cargo test --workspace                          # 61 tests (perfil, muestreador, registro y procesos reales incluidos)
cargo run --release -p overdrive-win --bin overdrive-probe -- --stability 7
cargo run -p overdrive-core --example gen_traces              # regenera la biblioteca de trazas
cargo run -p overdrive-core --example inspect_trace -- <nombre>  # veredicto ventana a ventana
```

## Fase 5: motor de transacciones y política de seguridad

### Piezas

| Pieza | Dónde | Qué hace |
|---|---|---|
| Acciones | `core::engine::{Action, actions}` | Conjunto cerrado y serializable: valor de registro (incluye IFEO, preferencia de GPU, Game DVR…), tipo de arranque de un servicio, plan de energía, prioridad del proceso, EcoQoS y suspensión. Cada una sabe `capture`, `apply`, `verify_applied`, `revert(before)`, `verify_reverted` y `changed_by_other`, y todas son idempotentes. |
| Journal | `core::engine::journal` | SQLite en WAL con `synchronous=FULL`. El esquema es **`docs/architecture/journal.sql` incluido literal**, que sigue siendo la única fuente. Cada escritura se confirma por separado, así que el estado original está en disco **antes** de aplicar. |
| Motor | `core::engine::tx::Engine` | `apply` (todo verificado o rollback), `revert`, `recover` (al arrancar), `revert_all` (reset global con diff final) y `release_processes` (primer plano / "Reanudar todo"). Expone **todos** los puntos de fallo (`CrashPoint`) para los tests. |
| Política | `core::safety` | Anti-cheats (exe conocidos y firmas en la carpeta del juego). Si no se sabe, se trata como kernel. Decisión por proceso: **un proceso desconocido no se toca**; solo se bajan apps de fondo conocidas y solo se suspenden actualizadores conocidos. Guard previo a cualquier acción sobre procesos. |
| Windows | `win::system_ops::WinSystem` | Registro (incluida la colmena de otro usuario), SCM, `PowerSetActiveScheme`, `SetPriorityClass`, EcoQoS (`SetProcessInformation`), `NtSuspendProcess`/`NtResumeProcess`. Cada operación sobre un proceso comprueba antes PID + hora de creación. |

### Garantías y cómo se demuestran

| Garantía | Test |
|---|---|
| Aplicar y revertir deja el sistema **idéntico**, incluidas las claves de registro vacías que se crearon al aplicar | `apply_then_revert_restores_everything_exactly` (fake) · `registry_cycle_on_the_real_registry` (registro real) |
| Un fallo en cualquier paso revierte los anteriores | `a_failing_step_rolls_back_the_previous_ones` |
| Un cambio que "no hace nada" se detecta al verificar | `a_change_that_silently_does_nothing_is_caught_by_verification` |
| No se pisa un valor que otro cambió después | `a_value_changed_by_someone_else_is_not_overwritten` |
| Los fallos al revertir se informan y se reintentan | `revert_failures_are_reported_precisely_never_silent` |
| **Matar el proceso en cualquier punto → recuperación completa** | Fake: 34 puntos al aplicar y 16 al revertir con los 6 tipos de acción. **Real: 26 `abort()` de un proceso hijo sobre el registro de verdad**, cada uno seguido de una recuperación en otro proceso (`real_process_killed_at_every_point`) |
| Los persistentes aceptados se mantienen; los que quedaron a medias se deshacen | `recovery_keeps_accepted_persistent_changes_but_undoes_half_applied_ones` |
| El reset global no deja diferencias | `global_reset_reverts_everything_and_verifies_no_diff` |
| **Un proceso suspendido se reanuda en todos los caminos de salida**: fin de sesión, rollback, timeout, caída + recuperación, reset, primer plano y proceso que muere solo | `a_suspended_process_is_resumed_on_every_exit_path` · `real_process_priority_ecoqos_suspend_and_resume` (proceso real) |
| PID reutilizado → no se toca | `pid_reuse_is_detected` (fake) y prueba equivalente en real |
| **No se abre nunca un handle a un juego con anti-cheat de kernel** (ni a los procesos del anti-cheat) | `kernel_anticheat_game_is_never_opened`: el fake registra cada PID abierto |
| Política de procesos de fondo (16 casos) y detección de anti-cheat | `background_policy_touches_only_known_safe_processes`, `anticheat_detection` |

### Decisiones

- **EcoQoS en Windows 10:** se puede *fijar* pero no *consultar*. El estado se marca como "no verificable en esta versión", la verificación se basa en que la llamada a Windows tuvo éxito, y al revertir se vuelve a "que decida Windows". Sin esto, el motor lo deshacía siempre, que es seguro pero lo dejaba inútil en Windows 10.
- **Las acciones sobre procesos son pasos normales del journal** (se ha quitado la tabla `background_actions`), así que tienen las mismas garantías write-ahead y de recuperación.
- **Un persistente a medias se revierte entero.** Completarlo sería adivinar.

### Criterio de terminado de la Fase 5

- [x] Journal, máquina de estados de transacciones, aplicar/verificar/revertir idempotentes para cada tipo de estado de la §5.7 que existe hoy
- [x] Recuperación tras caída, reset global, política de suspensión y comprobaciones de anti-cheat
- [x] Test de matar en cada paso: **con muertes reales del proceso sobre el registro real** en lugar de una VM
- [x] Un proceso suspendido se reanuda en todos los caminos de salida
- [x] Nunca se abre un handle a un proceso protegido por anti-cheat
- [ ] **Pendiente:**
  - **Tipos de estado que llegan en fases siguientes:** tareas programadas (Fase 6, con la reducción de ruido de fondo), perfiles de driver y parches de ficheros (Fase 7).
  - **Servicios y plan de energía:** los tests en vivo son de solo lectura, porque cambiarlos de verdad necesita administrador y tocaría tu sistema. La escritura está cubierta con el fake y se verificará dentro del servicio (LocalSystem).
  - **VM con Windows 10 y 11 con apagón simulado:** sigue siendo deseable antes de 1.0.

## Fase 4: telemetría y clasificador de cuellos de botella

### Piezas

| Pieza | Dónde | Qué hace |
|---|---|---|
| Parser de PresentMon | `core::telemetry::trace` | Lee CSV de PresentMon 1.x (`MsBetweenPresents`, `MsGPUActive`) y 2.x (`FrameTime`, `GPUBusy`) por cabecera, también línea a línea para el streaming. Los `NA` pasan a `None` y los fotogramas de *frame generation* se descartan. |
| Métricas | `core::telemetry::metrics` | FPS medios (fotogramas / tiempo), 1 % y 0,1 % low = 1000 / P99 (P99,9), p50/p95/p99, desviación y tirones (más de 2× la mediana móvil de 30 fotogramas **y** al menos 8 ms más lentos). |
| Clasificador | `core::telemetry::classify` | Ventanas de 5 s → carga/menú/partida → reglas → histéresis → veredicto con confianza y evidencias. |
| Biblioteca de trazas | `core/tests/traces/` | 11 escenarios en formato PresentMon con sus contadores de sistema y la etiqueta esperada (`manifest.json`). |
| PresentMon | `win::telemetry::PresentMonSource` | Lanza PresentMon 2.x para un PID (ETW, sin inyección), lee su stdout en un hilo y entrega fotogramas sin bloquear. |
| Muestreador | `win::telemetry::SystemSampler` | Contadores PDH a 1 Hz (commit de RAM, `Page Reads/sec`, cola de disco, `% Performance Limit`, VRAM dedicada) y estado de batería. |

### Reglas del clasificador (umbrales configurables en `Thresholds`)

1. **Carga:** grupo de 3 o más fotogramas de más de 250 ms en 3 s, o una congelación de más de 2 s, detectados a nivel de fotograma. Un tirón aislado **no** es una carga: sigue contando como tirón.
2. **Menú:** GPU casi parada (ratio < 0,20) y fotogramas muy regulares (CV < 0,03).
3. **Temperatura/energía:** batería o `% Performance Limit` < 85.
4. **Memoria:** RAM o VRAM por encima del 90 % **y** al menos la mitad de los tirones coinciden con ráfagas de fallos de página. Tener la memoria alta por sí solo no es un cuello de botella.
5. **Disco:** al menos la mitad de los tirones coinciden con picos de la cola de disco.
6. **GPU Busy / frametime:** ≥ 0,90 → GPU; ≤ 0,75 → CPU; entre medias → mixto; sin GPU Busy → desconocido.
7. **Histéresis:** la línea de tiempo solo cambia tras 3 ventanas seguidas que coinciden; la etiqueta inicial es la mayoría de las 3 primeras. Si ninguna etiqueta tiene mayoría, el veredicto es "mixto".

### Resultados

| Traza | Esperado | Obtenido |
|---|---|---|
| gpu_bound_60fps | GPU | GPU (100 %) |
| cpu_bound_120fps | CPU | CPU (100 %) |
| mixed_boundary | mixto | mixto (100 %) |
| memory_bound_8gb | memoria | memoria (100 %) |
| vram_high_but_cpu_bound | CPU | CPU (100 %) |
| io_bound_hdd | disco | disco (100 %) |
| thermal_laptop | temperatura | temperatura (67 %: los primeros 12 s aún no hay limitación) |
| battery_laptop | temperatura/energía | temperatura/energía (100 %) |
| loading_menu_then_gpu | GPU | GPU (100 %) |
| cpu_bound_with_gpu_blip | CPU | CPU (100 %; la escena de GPU de 5 s no cambia el veredicto) |
| no_gpu_busy_v1 | desconocido | desconocido (100 %) |

**Consumo medido en la máquina real (Windows 10, i5-11400F):**
- Muestreador: **0,061 % de un núcleo** a 2 Hz, el doble que en producción, medido en ciclos exactos de su propio hilo.
- Coste único al empezar la sesión: unos 440 ms de CPU, por la carga del proveedor de contadores de GPU.
- Parser: **0,9 µs por fotograma**, un 0,03 % de un núcleo a 300 FPS.

### Criterio de terminado de la Fase 4

- [x] Integración con PresentMon/ETW (spawn, streaming y parseo) probada con el mismo camino de stdout
- [x] Métricas, clasificador con histéresis y exclusión de cargas y menús
- [x] Biblioteca de trazas y tests: el clasificador acierta en **todas** las trazas
- [x] Consumo de la telemetría dentro del presupuesto (§4.3) en lo medible aquí: muestreador y parser
- [ ] **Pendiente:**
  - Las trazas son **sintéticas**: reproducen la firma de cada cuello de botella, pero falta añadir capturas reales de partidas, sobre todo de los PCs de los amigos.
  - PresentMon todavía no está empaquetado (se incluirá junto al servicio en `presentmon\PresentMon.exe`) y su consumo propio no se ha medido.
  - La captura ETW real necesita permisos de administrador; el servicio, que corre como LocalSystem, los tendrá.

`overdrive-probe` es **de solo lectura**: imprime en JSON el perfil, los problemas ordenados por impacto y los flujos del BIOS Coach.

## Fase 3: perfilador de hardware

### Fuentes de datos (todas en modo usuario, sin drivers de kernel)

| Dato | Fuente | Confianza |
|---|---|---|
| CPU: modelo, núcleos, hilos y reloj | WMI `Win32_Processor` | exacta |
| Núcleos P/E | `GetLogicalProcessorInformationEx` (EfficiencyClass) | exacta |
| Generación de CPU | Nombre comercial ("i5-11400F" → 11) | estimada |
| Límite de rendimiento / throttling | PDH `% Performance Limit` (1 muestra de 1 s) | valor exacto, veredicto estimado |
| RAM: módulos, velocidad configurada y velocidad JEDEC | WMI `Win32_PhysicalMemory` (SMBIOS) | exacta |
| RAM: velocidad nominal XMP/EXPO | **Número de pieza** (Corsair, G.Skill, Kingston, Crucial, TeamGroup…) | estimada |
| Canales de RAM | Etiquetas de las ranuras (`ChannelA-DIMM0`, `DIMM_B1`…) | estimada, nunca exacta |
| Límite de RAM de la plataforma | Generación Intel + chipset de la placa (p. ej. 10.ª gen en placas no Z) | estimada |
| GPU, VRAM e integrada/dedicada | DXGI + fabricante/VRAM | VRAM exacta, tipo estimado salvo NVIDIA |
| Driver de GPU (versión, fecha y antigüedad) | WMI `Win32_VideoController` | exacta |
| Enlace PCIe actual/máximo | cfgmgr32 `DEVPKEY_PciDevice_*` | exacta |
| Discos: tipo, bus, salud y espacio libre | WMI `MSFT_PhysicalDisk` / `MSFT_Partition` / `Win32_LogicalDisk` | exacta |
| Portátil y corriente/batería | Chasis SMBIOS + `GetSystemPowerStatus` | exacta |
| Windows y ajustes de juego (Game Mode, HAGS, Game DVR, VRR, reinicio pendiente) | Registro | exacta |
| Placa y BIOS | WMI `Win32_BaseBoard` / `Win32_BIOS` | exacta |
| Estabilidad (BIOS Coach) | Event Log: WHEA-Logger, Kernel-Power 41, BugCheck 1001 | exacta |

### Decisiones importantes

- **Velocidad XMP por número de pieza.** SMBIOS solo da la velocidad JEDEC (p. ej. 2133) y no la del perfil XMP (3200). Leer el SPD exigiría un driver de SMBus, que está prohibido. Por eso la velocidad nominal se deduce del número de pieza y se marca como *estimada*.
- **No recomendar lo imposible.** Las placas Intel de 6.ª a 10.ª generación sin chipset Z (B460, H410…) y las H510 con 11.ª generación limitan la RAM. En ese caso no se envía al usuario a la BIOS: se le explica que ya va al máximo (`ram_platform_locked`).
- **Portátiles:** casi ninguna BIOS de portátil ofrece XMP, así que la RAM lenta se explica en vez de mandar al usuario a la BIOS.
- **Colmena del usuario:** el servicio corre como LocalSystem, y ahí HKCU es la colmena de SYSTEM. Los ajustes de cada usuario se leen de `HKEY_USERS\<SID>` (`UserHive::Sid`).
- **Fallos aislados:** si una fuente falla, sus campos quedan como `unknown` y el error se registra en `warnings`. El perfil nunca falla entero.
- **Rendimiento:** la muestra de PDH de 1 s corre en otro hilo, en paralelo con WMI. En la máquina de desarrollo el perfil completo tarda **1,5–2,3 s**.

### Criterio de terminado de la Fase 3

- [x] Código nativo de producción para la §5.1, con fuente y confianza por campo
- [x] Tier, problemas estructurales ordenados y BIOS Coach (detección, flujo guiado por fabricante, verificación tras reiniciar y vigilancia de estabilidad)
- [x] Fakes (3 presets, uno por tier) y tests: 18 unitarios y de presets, más 1 de integración contra el hardware real
- [x] Sin drivers de kernel; menos de 3 s (verificado en el test de integración)
- [x] Lo desconocido se informa como `unknown`; el test de integración comprueba que `unknown` ⇔ `value: null` en todo el perfil
- [ ] **Windows 11:** de momento solo se ha validado en Windows 10 22H2 (la máquina de desarrollo). La lógica específica de Windows 11 está cubierta por tests con fakes, pero falta ejecutar `overdrive-probe` en un equipo real con Windows 11

### Limitaciones conocidas

- Resizable BAR: no se puede leer sin driver; queda como `unknown`.
- Salud del disco: solo el `HealthStatus` de Windows, no los atributos SMART individuales.
- Throttling: una sola muestra en reposo. La medición fiable bajo carga llegará con la telemetría de sesión (Fase 4).
