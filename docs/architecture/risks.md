# Registro de riesgos

Probabilidad e impacto: **A**lto / **M**edio / **B**ajo.

| # | Riesgo | P | I | Mitigación | Fase |
|---|---|---|---|---|---|
| R1 | **Curva de aprendizaje de Rust** ralentiza el proyecto | A | M | `overdrive-core` sin `unsafe` ni Win32; código difícil aislado en `overdrive-win`; la UI sigue en web; cada puerto con tests y fakes | 3+ |
| R2 | **El servicio LocalSystem es un vector de escalada de privilegios** | M | A | Pipe con DACL y verificación de firma del cliente, métodos cerrados sin comandos ni rutas libres, Rust, fuzzing del parser IPC, revisión de seguridad antes de 1.0 | 1, 5, 8 |
| R3 | **Falsos positivos de antivirus y SmartScreen** (suspende procesos, edita el registro) | A | A | Firma de código desde la primera beta, sin técnicas "sospechosas" (inyección, drivers), enviar builds a Microsoft para análisis, construir reputación con un canal beta. Comprobar la elegibilidad y el coste de Azure Trusted Signing o de un certificado OV para un desarrollador individual en España | 8 (decidir en 3) |
| R4 | **Anti-cheat marca o bloquea al usuario** | B | A | Sin handles al proceso del juego en títulos protegidos, solo tweaks fuera del proceso, base de datos de anti-cheat, PresentMon basado en ETW (lo usan herramientas de NVIDIA, AMD e Intel); probar con los títulos más jugados del grupo de amigos | 5, 6 |
| R5 | **Los tweaks no aportan ganancia medible** en muchos PCs | A | M | Es un resultado aceptable y honesto: el valor está en BIOS Coach, el arreglo de GPU híbrida, el limitador de FPS, el ruido de fondo y los consejos estructurales. El A/B lo demuestra en vez de prometerlo | 6 |
| R6 | **El A/B es ruidoso** (escenas distintas, shaders compilándose, temperatura) | A | M | Protocolo con duración mínima, exclusión de cargas y menús, varias pasadas, intervalo de confianza, veredicto `inconclusive` permitido, benchmarks integrados de los juegos cuando existan | 4, 6 |
| R7 | **WebView2 supera el presupuesto de memoria** | M | B | Destruir la ventana al minimizar, tray sin WebView, medición en CI | 2, 8 |
| R8 | **Cambios de Windows o de drivers rompen primitivas** (claves movidas, ajustes renombrados) | M | M | `applies_when.os_versions`, `verify_applied` detecta que no hizo efecto, Drift Guardian, matriz de VMs Win10/Win11 en CI | 3+ |
| R9 | **Cobertura incompleta de NVAPI DRS / ADLX** (GPUs antiguas, iGPU Intel) | A | B | Detección de capacidades por GPU; si no hay soporte, se recomienda el ajuste en el juego o en el panel del driver | 7 |
| R10 | **PresentMon:** dependencia externa y cambios de formato | M | M | Fijar una versión, ejecutarlo solo durante sesiones, aislarlo tras `TelemetrySource`; integrar el consumo de ETW directamente en el servicio cuando sea viable | 4 |
| R11 | **BIOS Coach:** el usuario se pierde en la BIOS o el sistema queda inestable | M | A | Instrucciones por fabricante, recomendar el perfil oficial o un paso por debajo (nunca overclock), vigilar la estabilidad tras el cambio y guiar el "cargar valores por defecto"; en el modo Fácil solo con confirmación explícita | 3 |
| R12 | **Migración desde v0.2.x** deja tweaks antiguos aplicados | M | M | El instalador 1.0 ejecuta la reversión de la v0.2.x y detecta tweaks antiguos mediante `foreign_harmful` | 8 |
| R13 | **Proyecto en solitario: alcance demasiado grande** | A | A | Fases con criterio de terminado; prioridad a lo que más ayuda a los amigos: GPU híbrida, BIOS Coach, Game DVR, ruido de fondo, limitador de FPS. Comunidad y compartir quedan al final | todas |
| R14 | **Privacidad** en "Ayuda a un amigo" y en los presets | B | M | Redacción automática (sin usuario, rutas ni lista de procesos), vista previa de lo que se envía, todo opt-in | 8 |
