# Overdrive 1.0 — motor en Rust

| Crate | Qué es |
|---|---|
| `overdrive-core` | Lógica pura, sin Win32: perfilador (módulo A), tier, problemas estructurales, BIOS Coach, telemetría y clasificador (módulo B), catálogo (C), motor de transacciones (D), orquestador de sesión y protocolo A/B (E), política de seguridad (F) y Drift Guardian (I). Testeable con fakes (`testing::FakeSource`, `engine::fake::FakeSystem`) y trazas grabadas. |
| `overdrive-win` | Implementación real para Windows 10/11. Es el único crate con `unsafe` (`src/sys.rs`, `src/telemetry.rs`, `src/system_ops.rs`, `src/processes.rs`). Incluye el binario `overdrive-probe`. |

```bash
cargo test --workspace                          # 132 tests (perfil, muestreador, registro, tareas, procesos, archivos y NVAPI reales incluidos)
powershell -File tools/footprint.ps1             # presupuesto de consumo en reposo (§4.3)
cargo run --release -p overdrive-win --bin overdrive-probe -- --stability 7
cargo run -p overdrive-core --example gen_traces              # regenera la biblioteca de trazas
cargo run -p overdrive-core --example inspect_trace -- <nombre>  # veredicto ventana a ventana
```

## Fase 8 (en curso): compartir, presets de la comunidad y release

### Decisiones tomadas

- **Firma de código:** de momento sin certificado. Azure Artifact Signing no admite desarrolladores individuales fuera de EE. UU. y Canadá, y un certificado OV cuesta unos 200–400 €/año. SmartScreen seguirá avisando.
- **Backend de la comunidad:** GitHub. Compartir abre un issue ya rellenado en el navegador y el usuario lo envía con su cuenta; una Action verifica y agrega. No hay servidor propio.
- **Actualizaciones:** GitHub Releases (pendiente).
- **El servicio nunca usa la red.** Solo la ventana de la app descarga el archivo público de presets, y únicamente con el opt-in activado.

### Piezas

| Pieza | Dónde | Qué hace |
|---|---|---|
| Núcleo de compartir | `core::share` | Identidad por instalación (ed25519), clase de hardware anónima (tier, portátil, fabricante de GPU y hash de CPU+GPU+RAM, sin modelos), tarjetas, mediciones, agregación de presets y códigos de "Ayuda a un amigo". Códigos `PREFIJO.base64url(deflate(json))`, con límite de tamaño y protección contra bombas zip. |
| Tarjeta antes/después | `share.card`, `ui/share.ts` | Solo desde un informe con línea base medida. SVG autónomo exportable a PNG; el submuestreo usa el máximo de cada tramo para que **un tirón nunca desaparezca** de la gráfica. Lleva un id de verificación, y `share.verifyCard` detecta cualquier retoque. |
| Mediciones para presets | `share.measurements` | Solo veredictos A/B finales (`kept`, `no_benefit`, `harmful`). La vista previa es **exactamente** el JSON que se publica. |
| Agregación | `core::share::aggregate`, `.github/workflows/presets.yml` | Firmas verificadas y cada instalación cuenta una vez. Un preset solo muestra una ganancia con **≥ 5 instalaciones independientes**, ≥ 80 % de acuerdo y mediana ≥ 3 %; si no, queda "midiéndose". Se publica en `presets/community.json` y se modera cerrando issues. |
| Ayuda a un amigo | `help.*` | Petición anonimizada: modelos de las piezas, problemas estructurales y, por juego, plan e historial medido; nunca rutas, usuario ni procesos. La respuesta del amigo **solo puede activar o desactivar lo que el plan ya ofrece**, se vuelve a filtrar en el PC que la aplica y se rechaza si no responde a una petición hecha desde ese PC. |
| Shell | `ui/src-tauri` | `open_issue` solo abre URLs de nuevo issue de este repositorio; `fetch_presets` solo descarga el archivo fijo, máximo 2 MB. |
| Presupuesto de consumo | `tools/footprint.ps1`, `ci.yml` | Mide el servicio en reposo (modo dry-run) y falla si supera **15 MB o el 0,1 % de CPU**. |
| CI | `.github/workflows/ci.yml` | Clippy sin avisos, tests de Rust, tests de la UI (contrato con el servicio real) y presupuesto de consumo, en Windows. |

### El presupuesto de consumo encontró un problema real

La primera medición dio **0,19 % de CPU en reposo**, casi el doble del límite. Midiendo por partes (`overdrive-win/examples/scan_cost.rs`):
- El servicio sin vigilar procesos gasta prácticamente un 0 %.
- El listado completo de procesos cuesta 7,5 ms (ventanas + sesiones de audio WASAPI) y se hacía cada 2 s.
- Además, al arrancar todos los procesos contaban como "nuevos" durante 90 s, y los ejecutables auxiliares nunca quedaban resueltos.

Arreglo:
- Una instantánea barata (1,4 ms, sin abrir procesos y solo de la sesión del usuario) decide si hace falta el listado completo.
- Cada proceso nuevo se identifica una sola vez.
- En reposo se comprueba cada 3 s.

**Resultado: 0,026 % de CPU y 6,7 MB.** El juego se detecta como mucho 3 s después de arrancar. ETW (§4.3), que no tendría coste en reposo, sigue pendiente.

### Pendiente

- [ ] Actualizador: manifiesto firmado (ed25519), despliegue escalonado y vuelta atrás.
- [ ] Workflow de release y paso de firma de código preparado para cuando haya certificado.
- [ ] Exportar diagnóstico anonimizado y rotación de logs (§8).
- [ ] Instalación limpia en máquinas virtuales Windows 10 y 11 (criterio de terminado de la fase 8).
- [ ] Subir los cambios a GitHub para que funcionen la CI, los presets y las releases.

## Fase 7: parcheador de configuración de juegos y perfiles de driver

### Piezas

| Pieza | Dónde | Qué hace |
|---|---|---|
| Edición de texto | `core::config` | Formatos `ini` (`[Sección]` + `clave=valor`, Unreal y la mayoría de juegos de PC) y `colon` (`clave:valor`, Minecraft). Detecta la codificación por el BOM (UTF-8, UTF-8 con BOM, UTF-16 LE/BE) y la conserva. Respeta el salto de línea de cada línea y el espaciado alrededor del `=`. Solo cambia el valor de las claves pedidas. Es **determinista**: con la misma entrada produce exactamente los mismos bytes. Si un archivo no es texto válido en su codificación, lo rechaza: nunca lo "arregla". |
| Primitiva `FilePatch` | `core::engine` | Los bytes originales van **dentro del journal** (base64, máximo 1 MiB) antes de escribir nada, con la misma garantía write-ahead que el registro. Nunca crea archivos y se niega a tocar los de solo lectura. Al revertir: si el archivo sigue siendo exactamente lo que escribimos, restaura el original **byte a byte**. Si el juego lo reescribió, deshace solo nuestras claves y conserva lo que el jugador cambió dentro del juego. Si el jugador cambió nuestras claves, son suyas y no se tocan. |
| Primitiva `DriverSetting` | `core::engine`, `win::nvapi` | Ajuste de driver por ejecutable, independiente del fabricante: `MaxFrameRate` (FRL_FPS) y `PowerMode` (PREFERRED_PSTATE). Si el exe no tiene perfil, se crea `Overdrive - <exe>`, y se borra al revertir si queda vacío. No se pisa un valor que cambió otro programa. |
| NVAPI DRS | `win::nvapi` | Carga dinámica de `nvapi64.dll` (sin SDK). IDs de función, estructuras y códigos tomados de las cabeceras públicas de NVIDIA; los tamaños se comprueban **en tiempo de compilación** (esa comprobación cazó un error de cálculo en la estructura V4). Cada operación abre y cierra su propia sesión DRS. |
| Escritura atómica | `win::system_ops::files` | Archivo temporal en la misma carpeta, `sync_all` y `ReplaceFileW`, que conserva atributos, ACL y fecha de creación. Si el destino no existe, `MoveFileExW(WRITE_THROUGH)`. |
| Base de datos por juego | `catalog/games/*.toml` + `core::games` | Por juego: rutas (`{appdata}`, `{localappdata}`, `{documents}`, `{install}`), claves que el juego lee de verdad, valor seguro, impacto visual (`none`/`low`/`visible`), evidencia, si la nube o el juego reescriben el archivo y si usa un ejecutable compartido. `lower_is_faster`: un valor numérico solo se baja, nunca se sube. **Primeras entradas:** Minecraft Java (`options.txt`) y 13 títulos protegidos (CS2, Fortnite, Rocket League, Apex, PUBG…). |
| Tweaks nuevos | `catalog/tweaks.toml` | `game_config` (ajustes por juego; en modo Fácil solo los probados con impacto visual nulo o bajo), `driver_power` (NVIDIA a máximo rendimiento solo para ese exe, `version_dependent`), `unreal_engine_ini` (regla genérica de Unreal, **solo Experto**, con vista previa del diff) y `frame_cap`, que ya funciona. |
| Limitador inteligente | `catalog::frame_cap` | Solo si la medición base dice que el juego está limitado por la CPU. Límite = 1 % low × 0,97, redondeado a múltiplos de 5 para que sea estable, y solo si queda ≥ 60 FPS y deja margen. Orden: limitador del driver → limitador del propio juego (base de datos) → recomendar el del juego. Se juzga con la métrica nueva `pacing` (dispersión del frametime), no con los FPS medios. |
| Minecraft | `svc::library` | Se detecta por `%APPDATA%\.minecraft`, y su "carpeta de instalación" es el Java que trae el launcher: un `javaw.exe` de ahí es Minecraft y cualquier otro Java no. Como `javaw.exe` es compartido, IFEO y los perfiles de driver se niegan (`shared_executable`) y el límite de FPS va por `options.txt`. |
| Vista previa | `catalog::preview`, `PlanItem.preview`, UI | En modo Experto, cada parche muestra archivo, clave, valor actual → valor nuevo. |

### Garantías y cómo se demuestran

| Garantía | Test |
|---|---|
| **El archivo original vuelve idéntico byte a byte** (BOM, CRLF, última línea sin salto) | `apply_then_revert_restores_everything_exactly` y `minecraft_easy_mode_patches_only_proven_low_impact_settings` (fake) · `real_file_patch_is_atomic_and_reverts_byte_identical` (disco real) |
| **Matar el proceso en cualquier punto → recuperación completa**, también con parches de archivo y ajustes de driver | Los tests de "matar en cada punto" de la Fase 5, ahora con 11 acciones (46 puntos al aplicar y 22 al revertir) |
| El juego reescribe el archivo durante la partida → se conserva lo que cambió el jugador | `the_game_rewriting_the_file_mid_session_keeps_the_players_changes`, `values_the_player_changed_after_us_are_theirs` |
| Solo lectura respetado: ni se escribe ni se quita el atributo | `read_only_files_are_respected` (fake y disco real) |
| **Los títulos protegidos nunca se tocan**, aunque la detección de anti-cheat falle en la máquina | `protected_titles_are_never_touched`: todos los tweaks × todos los títulos protegidos × Fácil/Experto, con todos los archivos presentes · `the_orchestrator_never_patches_a_protected_game_end_to_end` |
| El ajuste de driver vuelve exactamente a lo que había, y no se pisa un cambio ajeno | `driver_settings_revert_to_exactly_what_was_there` |
| **NVAPI real**: crear perfil → FPS máximos 117 + máximo rendimiento → revertir → el perfil desaparece | `real_nvidia_driver_profile_round_trip` (GTX 1660 SUPER, driver 621.14, sin administrador) |
| Con tu `options.txt` real: plan correcto y el archivo no cambia al planificar | `real_minecraft_plan_is_previewed_without_touching_anything`: Fácil propone `prioritizeChunkUpdates` 2→0 y `biomeBlendRadius` 2→1; Experto además `entityShadows` |

### Decisiones

- **Copia del original dentro del journal**, en vez de la tabla `file_backups` + carpeta de copias prevista en la Fase 1. Así la copia es durable en el mismo commit que el paso: no hay una segunda escritura que sincronizar ni copias huérfanas. La tabla queda sin uso.
- **Protegido = cualquier anti-cheat**, también los de modo usuario como VAC. Basta con que la base de datos **o** la detección digan que un juego está protegido para que sus archivos no se toquen. Los ajustes a nivel de sistema que no tocan el juego (driver, energía, GPU preferida) siguen permitidos.
- **No se crean archivos de configuración.** Si un juego no tiene `Engine.ini`, la regla de Unreal no se aplica.

### Criterio de terminado de la Fase 7

- [x] Formato de la base de datos por juego + primeras entradas
- [x] Parcheador atómico (§5.5): copia del original, escritura atómica, codificación y saltos de línea conservados, detección de reescritura por el juego, solo lectura respetado
- [x] Integración con NVAPI DRS, probada en el driver real
- [x] Los originales vuelven idénticos byte a byte al revertir
- [x] Los títulos protegidos nunca se tocan
- [ ] **Pendiente:**
  - **ADLX (AMD):** no implementado. No hay una GPU AMD para probarlo, y llamar a una API de drivers sin probarla puede tumbar el servicio. Con AMD, el limitador usa el del juego (si está en la base de datos) o lo recomienda. Los ajustes de driver dicen "Necesita una gráfica NVIDIA".
  - **Base de datos pequeña:** solo Minecraft tiene ajustes. Cada juego nuevo necesita comprobar sus claves reales; lo ideal es añadir los que usen tus amigos.
  - La vista previa del diff está probada con el motor simulado y por tipos, pero no se ha revisado a ojo en la app.

## Fase 6: catálogo, protocolo A/B, orquestador y Drift Guardian

### Piezas

| Pieza | Dónde | Qué hace |
|---|---|---|
| Catálogo | `catalog/tweaks.toml` + `core::catalog` | 11 tweaks como **datos**: alcance, momento (antes del arranque / al arrancar), riesgo, evidencia, si es compatible con anti-cheat, cómo se mide y orden del experimento. Cada id tiene un constructor que **lee el estado actual** y devuelve las acciones exactas, o explica por qué no aplica o si ya estaba bien. Las cadenas compartidas (`DirectXUserGlobalSettings`, `UserGpuPreferences`) se fusionan sin pisar otras opciones. |
| Primitivas nuevas | `core::engine`, `win::system_ops` | Tareas programadas (COM Task Scheduler) y lista de planes de energía (`PowerEnumerate`). |
| Protocolo A/B | `core::experiment` | Línea base, después **un tweak cada vez** frente a la configuración anterior. Welch con IC del 99 %, ganancia mínima del 3 %, equivalencia de ±3 %, 3 a 8 partidas por configuración, alternando A y B cuando hay dudas. **Sin estado oculto**: todo se deduce de las sesiones y veredictos del journal. Las sesiones con otro contexto (driver, ajustes del juego, energía) no se comparan. |
| Orquestador | `core::session` | `arm` → `on_game_start` → `on_game_exit` (lo revierte todo, mide, decide, informa y ofrece mantener) → `accept_persistent`. Si el usuario arranca el juego sin pulsar Optimizar, los tweaks de "antes del arranque" no se cuentan en la medición. |
| Drift Guardian | `core::drift` | Revisa cada cambio persistente aceptado. Si una actualización lo deshizo, lo repara sola cuando es de riesgo bajo (si no, pregunta). Detecta regresiones de más del 10 % y apunta la causa probable: driver nuevo, ajustes del juego o fuente de energía. |
| Procesos | `win::processes` | Lista con identidad (PID + hora de creación), sesión, primer plano, ventanas visibles y **audio activo** (WASAPI, reproducción y captura) en todos los dispositivos. |

### Resultados

- **Prueba de extremo a extremo** (`tests/session.rs`): un portátil con dos gráficas y un juego con anti-cheat de kernel. Los efectos reales de cada tweak están ocultos para el orquestador, y el juego rinde según lo que de verdad está aplicado en el sistema simulado, con ruido entre partidas. El orquestador descubre los 7 efectos: se queda con la GPU dedicada, la energía, Game DVR y la telemetría; veta el aislamiento de fondo (dañino); y descarta la prioridad IFEO y la optimización de ventana. Al final solo quedan los 3 cambios persistentes aceptados, y "dejar mi PC como estaba" lo devuelve todo al original.
- **Robustez con 20 semillas de ruido:** **96,4 % de veredictos correctos**, **0 tweaks inútiles o dañinos conservados** y el dañino detectado las 20 veces. Los fallos son efectos reales pequeños (+5–6 %) marcados como "sin beneficio": el error prudente. Coste: mediana de 49 partidas para decidir los 7 tweaks. Las mejoras grandes se confirman en las primeras y quedan activas desde entonces.
  - La primera versión del protocolo solo acertaba el 77 % y conservaba 2 tweaks inútiles. Fallaba por no alternar A y B (la base se quedaba en 2 partidas) y por los falsos positivos de mirar los datos tras cada partida. Este test lo destapó; con una sola semilla parecía perfecto.
- **Drift:** una actualización de Windows que vuelve a activar Game DVR se detecta y se repara, y el reset global sigue dejando el sistema original.
- **En el PC real:**
  - Spotify estaba sonando desde un proceso y la política iba a bajar sus procesos hermanos. Ahora, si una app suena en cualquiera de sus procesos, no se toca ninguno.
  - Las tareas de telemetría del catálogo no existen en ese Windows (ya las había quitado su dueño): el constructor las salta.

### Criterio de terminado de la Fase 6

- [x] Catálogo declarativo con los ítems 1–6 (y 6b), cada entrada con evidencia, riesgo, alcance y tests
- [x] Orquestador de sesión, protocolo A/B, promoción a persistente e informes de sesión
- [x] Lo que no aporta se revierte solo y se informa; lo que empeora queda vetado en esa máquina
- [x] Una actualización simulada que deshace ajustes se detecta y se repara
- [ ] **Pendiente:**
  - ~~El limitador de FPS (ítem 2) espera a los perfiles de driver de la Fase 7.~~ Hecho en la Fase 7.
  - HAGS necesita reiniciar: se ofrece a mano en modo Experto, y falta el flujo de medición "antes/después del reinicio".
  - El servicio que conecta todo esto con la interfaz (IPC real) aún no existe.

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
