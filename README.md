# Overdrive — Game Optimizer

Aplicación de escritorio (Electron) que **analiza tu PC** y aplica **optimizaciones reversibles**
para jugar con mejor rendimiento. Pensada para Windows 10/11.

## Qué hace

1. **Analiza** el hardware y software: CPU, GPU, RAM, disco (SSD/HDD), sistema, plan de energía,
   procesos que más RAM consumen y **drivers desactualizados** (resaltando los "muy desactualizados").
2. Te deja elegir un **nivel de agresividad**: `Básica`, `Media` o `Agresiva`.
3. Genera un **plan** de optimizaciones con etiquetas de riesgo. Tú marcas qué aplicar.
4. Antes de tocar nada crea un **punto de restauración del sistema**.
5. Aplica **solo lo que confirmas** y te permite **revertir** los cambios.

## Filosofía de seguridad

- **Nada se aplica sin tu confirmación.** El plan muestra cada cambio, su riesgo y si requiere admin/reinicio.
- **Todo es reversible** (salvo la limpieza de temporales, que es inofensiva).
- Se crea un **punto de restauración** antes de aplicar.
- **Los drivers NO se instalan automáticamente.** La app detecta los desactualizados y abre la
  página de descarga **oficial** del fabricante (NVIDIA / AMD / Intel / Realtek) o Windows Update.
  Instalar drivers a ciegas es la causa nº1 de equipos inestables, así que esa decisión queda en ti.

## Optimizaciones incluidas

| Nivel | Optimización | Riesgo | Reversible |
|-------|--------------|--------|------------|
| Básica | Plan de energía Alto rendimiento | bajo | sí |
| Básica | Limpiar temporales | bajo | n/a |
| Básica | Activar Modo Juego | bajo | sí |
| Media | Efectos visuales → rendimiento | bajo | sí |
| Media | Desactivar Game DVR | bajo | sí |
| Media | Desactivar hibernación | medio | sí |
| Media | Desactivar Power Throttling | medio | sí |
| Agresiva | GPU Hardware Scheduling (HAGS) | medio | sí |
| Agresiva | Desactivar Network Throttling | medio | sí |
| Agresiva | Desactivar SysMain (solo SSD) | medio | sí |

## Requisitos

- Windows 10/11
- [Node.js](https://nodejs.org) 18+ (tienes v24)
- Para aplicar cambios de sistema, ejecutar **como administrador** (la app lo pide con UAC si hace falta).

## Uso en desarrollo

```bash
npm install
npm start
```

## Empaquetar a .exe

Con [electron-builder](https://www.electron.build):

```bash
npm run dist       # genera instalador NSIS + portable en dist/
npm run dist:dir   # solo la carpeta descomprimida (dist/win-unpacked), para probar
```

Produce en `dist/`:
- `Overdrive Setup <versión>.exe` — instalador (permite elegir carpeta, crea acceso directo).
- `Overdrive-<versión>-portable.exe` — ejecutable portable (sin instalación).

El icono se genera en `assets/icon.ico`. La app arranca como usuario normal y
**pide elevación (UAC) solo al aplicar** cambios que requieren administrador.

> Nota: no está firmado digitalmente, así que SmartScreen puede avisar la primera vez
> ("Más información → Ejecutar de todas formas"). Para distribución pública haría falta
> un certificado de firma de código.

## Estructura

```
src/
  main/
    main.js      # proceso principal Electron + IPC
    preload.js   # puente seguro (contextBridge)
    ps.js        # ejecutor de PowerShell (-EncodedCommand)
    system.js    # análisis de hardware/software/drivers (solo lectura)
    tweaks.js    # catálogo de optimizaciones (apply/revert/check)
  renderer/
    index.html   # UI
    styles.css   # estilo minimalista + animaciones
    renderer.js  # lógica de la interfaz
```

## Roadmap / ideas

- Empaquetado a `.exe` con `electron-builder`.
- Benchmark rápido antes/después.
- Perfiles por juego.
- Monitor de temperaturas y uso en tiempo real.
- Más tweaks con detección de estado y "diff" visual.

## Aviso

Modifica ajustes del sistema. Aunque son cambios estándar, reversibles y con punto de restauración,
úsalo bajo tu responsabilidad. Revisa el plan antes de aplicar.
