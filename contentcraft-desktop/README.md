# ContentCraft Desktop

Aplicación de escritorio para ContentCraft, construida con [Tauri 2](https://tauri.app). Guarda tu API Key de Anthropic en el **llavero del sistema operativo** (Keychain en macOS, Credential Manager en Windows, libsecret en Linux) — nunca en texto plano.

## Requisitos previos

| Herramienta | Versión mínima | Instrucciones |
|---|---|---|
| Rust + Cargo | 1.77+ | [rustup.rs](https://rustup.rs) |
| Tauri CLI | 2.x | `cargo install tauri-cli --version "^2"` |
| Node.js | 18+ | (solo para el script `tauri dev/build`) |

### Dependencias del sistema

**macOS** — nada extra (Xcode Command Line Tools ya incluye lo necesario).

**Windows** — instala [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) con las workloads C++ y Windows SDK.

**Linux (Debian/Ubuntu)**:
```bash
sudo apt install libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev \
  libdbus-1-dev pkg-config
```

## Desarrollo

```bash
cd contentcraft-desktop
cargo tauri dev
```

Abre la app en una ventana nativa. La API Key se guarda en el llavero del sistema.

## Build de producción

```bash
cargo tauri build
```

Genera el instalador en `src-tauri/target/release/bundle/`:
- macOS → `.dmg` y `.app`
- Windows → `.msi` y `.exe` (NSIS)
- Linux → `.deb`, `.rpm` y `.AppImage`

## Diferencias con la versión web

| Funcionalidad | Web (artifact) | Desktop |
|---|---|---|
| API Key | localStorage | Llavero del SO |
| Ventana nativa | No | Sí (drag region en header) |
| Offline parcial | No | Sí (interfaz carga sin internet) |
| Auto-update | — | Próximamente |
