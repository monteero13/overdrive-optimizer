# Overdrive 1.0 — motor en Rust

| Crate | Qué es |
|---|---|
| `overdrive-core` | Lógica pura, sin Win32: perfilador (módulo A), tier, problemas estructurales, BIOS Coach. Testeable con fakes (`testing::FakeSource`). |
| `overdrive-win` | Implementación real para Windows 10/11. Es el único crate con `unsafe`, aislado en `src/sys.rs`. Incluye el binario `overdrive-probe`. |

```bash
cargo test --workspace                          # 20 tests (incluye uno que perfila esta máquina de verdad)
cargo run --release -p overdrive-win --bin overdrive-probe -- --stability 7
```

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
