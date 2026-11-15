# Overdrive 1.0 — motor en Rust

| Crate | Qué es |
|---|---|
| `overdrive-core` | Lógica pura, sin Win32: perfilador (módulo A), tier, problemas estructurales, BIOS Coach, telemetría y clasificador (módulo B). Testeable con fakes (`testing::FakeSource`) y trazas grabadas. |
| `overdrive-win` | Implementación real para Windows 10/11. Es el único crate con `unsafe` (`src/sys.rs`, `src/telemetry.rs`). Incluye el binario `overdrive-probe`. |

```bash
cargo test --workspace                          # 41 tests (perfil y muestreador contra el hardware real incluidos)
cargo run --release -p overdrive-win --bin overdrive-probe -- --stability 7
cargo run -p overdrive-core --example gen_traces              # regenera la biblioteca de trazas
cargo run -p overdrive-core --example inspect_trace -- <nombre>  # veredicto ventana a ventana
```

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
