> This is the Spanish translation of the siggy README.
> Last updated against English commit: 8b5890e
> The [English version](../README.md) is authoritative. If this translation has drifted, trust the English.
> This translation is maintainer-provided and awaiting native-speaker review. Corrections welcome - see issue #353.

<p align="center">
  <img src="../siggy-banner.png" alt="siggy" width="600">
</p>

<p align="center">
  <a href="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml"><img src="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/johnsideserf/siggy/releases/latest"><img src="https://img.shields.io/github/v/release/johnsideserf/siggy" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/johnsideserf/siggy" alt="License: AGPL-3.0"></a>
  <a href="https://crates.io/crates/siggy"><img src="https://img.shields.io/crates/v/siggy" alt="crates.io"></a>
  <a href="https://johnsideserf.github.io/siggy/"><img src="https://img.shields.io/badge/docs-siggy-blue" alt="Docs"></a>
  <a href="https://ko-fi.com/johnsideserf"><img src="https://img.shields.io/badge/Ko--fi-Support%20siggy-ff5e5b?logo=ko-fi&logoColor=white" alt="Ko-fi"></a>
  <a href="https://x.com/siggyapp"><img src="https://img.shields.io/badge/follow-@siggyapp-000000?logo=x&logoColor=white" alt="Follow @siggyapp"></a>
</p>

<p align="center">
  <a href="../README.md">English</a>
  &nbsp;|&nbsp;
  <a href="README.da.md">Dansk</a>
  &nbsp;|&nbsp;
  <a href="README.de.md">Deutsch</a>
  &nbsp;|&nbsp;
  <b>Español</b>
  &nbsp;|&nbsp;
  <a href="README.fr.md">Français</a>
  &nbsp;|&nbsp;
  <a href="README.it.md">Italiano</a>
  &nbsp;|&nbsp;
  <a href="README.nl.md">Nederlands</a>
  &nbsp;|&nbsp;
  <a href="README.pt-BR.md">Português</a>
  &nbsp;|&nbsp;
  <a href="README.fi.md">Suomi</a>
  &nbsp;|&nbsp;
  <a href="README.sv.md">Svenska</a>
  &nbsp;|&nbsp;
  <a href="README.ru.md">Русский</a>
  &nbsp;|&nbsp;
  <a href="README.uk.md">Українська</a>
  &nbsp;|&nbsp;
  <a href="README.zh-CN.md">简体中文</a>
  &nbsp;|&nbsp;
  <a href="../TRANSLATING.md">Contribuye con una traducción</a>
</p>

Un cliente de mensajería Signal para terminal con estética IRC. Envuelve [signal-cli](https://github.com/AsamK/signal-cli) mediante JSON-RPC como backend de mensajería.

![Captura de pantalla de siggy](../screenshot.png)

## Instalación

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Binarios precompilados

Descarga la última versión para tu plataforma desde la página de [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (una sola línea):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Ambos scripts descargan el binario de la última versión y comprueban que signal-cli esté presente.

### Desde crates.io

Requiere Rust 1.70+.

```sh
cargo install siggy
```

### Compilar desde el código fuente

O bien clona el repositorio y compílalo localmente:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# El binario queda en target/release/siggy
```

## Requisitos previos

- [signal-cli](https://github.com/AsamK/signal-cli) instalado y accesible en el PATH (o configurado mediante `signal_cli_path`)
- Una cuenta de Signal vinculada como dispositivo secundario (el asistente de configuración se encarga de esto)

## Uso

```sh
siggy                        # Iniciar (usa el archivo de configuración)
siggy -a +15551234567        # Especificar la cuenta
siggy -c /path/to/config.toml  # Ruta de configuración personalizada
siggy --setup                # Volver a ejecutar el asistente de configuración inicial
siggy --demo                 # Iniciar con datos de prueba (no requiere signal-cli)
siggy --incognito            # Sin almacenamiento local de mensajes (solo en memoria)
```

En el primer arranque, el asistente de configuración te guía para localizar signal-cli, introducir tu número de teléfono y vincular tu dispositivo mediante un código QR.

## Configuración

La configuración se carga desde:
- **Linux/macOS:** `~/.config/siggy/config.toml`
- **Windows:** `%APPDATA%\siggy\config.toml`

```toml
account = "+15551234567"
signal_cli_path = "signal-cli"
download_dir = "/home/user/signal-downloads"
notify_direct = true
notify_group = true
desktop_notifications = false
inline_images = true
mouse_enabled = true
send_read_receipts = true
theme = "Default"
proxy = ""
```

Todos los campos son opcionales. `signal_cli_path` toma por defecto el valor `"signal-cli"` (localizado a través del PATH), y `download_dir` usa por defecto `~/signal-downloads/`. En Windows, indica la ruta completa a `signal-cli.bat` si no está en tu PATH.

### Imágenes integradas dentro de tmux

Fuera de tmux, siggy detecta automáticamente Kitty / iTerm2 / WezTerm / Ghostty y muestra los adjuntos como imágenes nativas en píxeles. Dentro de tmux hay que configurar dos cosas, porque tmux oculta el terminal exterior a siggy:

1. Indica a tmux que reenvíe las secuencias de escape desconocidas. Requiere tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Las versiones antiguas de tmux usan `set -g allow-passthrough all`.

2. Indica a siggy qué protocolo habla el terminal exterior (la detección automática solo ve tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # o iterm2 / sixel / halfblock
   ```

Si `SIGGY_IMAGE_PROTOCOL` no está definida, se ejecuta la detección automática habitual (correcta fuera de tmux; dentro recurre a halfblock). Sixel atraviesa tmux 3.4+ de forma nativa y no necesita la variable de entorno.

## Funcionalidades

- **Mensajería** -- Envía y recibe mensajes individuales y de grupo
- **Archivos adjuntos** -- Las vistas previas de imágenes se muestran integradas como arte de medios bloques (halfblock); los adjuntos que no son imágenes aparecen como `[attachment: filename]`
- **Enlaces clicables** -- Las URL y rutas de archivo son hipervínculos OSC 8 (clicables en terminales como Windows Terminal, iTerm2, etc.)
- **Indicadores de escritura** -- Muestra quién está escribiendo, con resolución del nombre del contacto
- **Sincronización de mensajes** -- Los mensajes enviados desde tu teléfono aparecen en la TUI
- **Persistencia** -- Almacenamiento de mensajes en SQLite con modo WAL; las conversaciones y los marcadores de lectura sobreviven a los reinicios
- **Seguimiento de no leídos** -- Contadores de mensajes no leídos en la barra lateral, con separador de "mensajes nuevos" en el chat
- **Notificaciones** -- Campana del terminal al recibir mensajes nuevos (configurable por mensajes directos/de grupo, con silenciado por chat) y notificaciones de escritorio del sistema operativo
- **Resolución de contactos** -- Nombres tomados de tu libreta de direcciones de Signal; los grupos se cargan automáticamente al iniciar
- **Reacciones a mensajes** -- Reacciona con `r` en modo Normal; selector de emojis con insignias de recuento (`👍 2 ❤️ 1`)
- **Responder / citar** -- Pulsa `q` sobre un mensaje enfocado para responder citando el contexto
- **Editar mensajes** -- Pulsa `e` para editar tus propios mensajes enviados
- **Eliminar mensajes** -- Pulsa `d` para eliminar de forma local o remota (tus propios mensajes)
- **Eliminar conversaciones** -- Usa `/delete` para eliminar localmente la conversación actual (rechaza las solicitudes de mensaje pendientes)
- **Búsqueda de mensajes** -- `/search <consulta>` con `n`/`N` para saltar entre resultados
- **@menciones** -- Escribe `@` en los chats de grupo para mencionar miembros con autocompletado
- **Selección de mensajes** -- Resaltado del mensaje enfocado al desplazarse; `J`/`K` para saltar entre mensajes
- **Confirmaciones de lectura** -- Símbolos de estado en los mensajes salientes (Enviando → Enviado → Entregado → Leído → Visto)
- **Mensajes temporales** -- Respeta los temporizadores de mensajes temporales de Signal; configurables por conversación con `/disappearing`
- **Gestión de grupos** -- Crea grupos, añade o quita miembros, renombra y abandona grupos mediante `/group`
- **Solicitudes de mensaje** -- Acepta o elimina mensajes de remitentes desconocidos
- **Bloquear / desbloquear** -- Bloquea contactos o grupos con `/block` y `/unblock`
- **Soporte de ratón** -- Haz clic en las conversaciones de la barra lateral, desplaza los mensajes y haz clic para posicionar el cursor
- **Temas de color** -- Temas seleccionables mediante `/theme` o `/settings`
- **Asistente de configuración** -- Puesta en marcha guiada en el primer arranque, con vinculación del dispositivo por código QR
- **Atajos estilo Vim** -- Edición modal (Normal/Insert) con movimiento completo del cursor
- **Autocompletado de comandos** -- Ventana emergente de autocompletado con Tab para los comandos slash
- **Panel de ajustes** -- Activa o desactiva notificaciones, barra lateral e imágenes integradas desde la propia aplicación
- **Diseño adaptable** -- Barra lateral redimensionable que se oculta automáticamente en terminales estrechos (<60 columnas)
- **Modo incógnito** -- `--incognito` usa almacenamiento en memoria; nada persiste al salir
- **Soporte de proxy** -- Configura un proxy TLS de Signal mediante el campo de configuración `proxy` para usarlo en redes restringidas
- **Modo demo** -- Prueba la interfaz sin signal-cli (`--demo`)

## Comandos

| Comando | Alias | Descripción |
|---|---|---|
| `/join <nombre>` | `/j` | Cambiar a una conversación por nombre de contacto, número o grupo |
| `/part` | `/p` | Salir de la conversación actual |
| `/delete` | | Eliminar la conversación actual (rechaza las solicitudes de mensaje pendientes) |
| `/attach` | `/a` | Abrir el explorador de archivos para adjuntar un archivo |
| `/search <consulta>` | `/s` | Buscar mensajes en la conversación actual (o en todas) |
| `/sidebar` | `/sb` | Mostrar u ocultar la barra lateral |
| `/bell [tipo]` | `/notify` | Activar o desactivar las notificaciones (`direct`, `group` o ambas) |
| `/mute [duración]` | | Silenciar o reactivar la conversación actual (p. ej. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Bloquear el contacto o grupo actual |
| `/unblock` | | Desbloquear el contacto o grupo actual |
| `/disappearing <dur>` | `/dm` | Definir el temporizador de mensajes temporales (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Abrir el menú de gestión de grupos |
| `/theme` | `/t` | Abrir el selector de temas |
| `/contacts` | `/c` | Explorar los contactos sincronizados |
| `/settings` | | Abrir el panel de ajustes |
| `/lock` | | Bloquear la sesión |
| `/lock-reset` | | Cambiar la frase de bloqueo (requiere la frase actual) |
| `/help` | `/h` | Mostrar el panel de ayuda |
| `/quit` | `/q` | Salir de siggy |

Escribe `/` para abrir la ventana de autocompletado. Usa `Tab` para completar y las teclas de flecha para navegar.

Para escribir a un contacto nuevo: `/join +15551234567` (formato E.164).

**¿Olvidaste tu frase de bloqueo?** Cierra siggy (o termina el proceso) y ejecuta `siggy --reset-lock`. Esto elimina el hash de la frase almacenada e imprime la ruta del archivo eliminado. El siguiente `/lock` establecerá una frase nueva.

## Atajos de teclado

La aplicación usa edición modal al estilo Vim con dos modos: **Insert** (predeterminado) y **Normal**.

### Globales (ambos modos)

| Tecla | Acción |
|---|---|
| `Ctrl+C` | Salir |
| `Tab` / `Shift+Tab` | Conversación siguiente / anterior |
| `PgUp` / `PgDn` | Desplazar los mensajes (5 líneas) |
| `Ctrl+Left` / `Ctrl+Right` | Redimensionar la barra lateral |

### Modo Normal

Pulsa `Esc` para entrar en modo Normal.

| Tecla | Acción |
|---|---|
| `j` / `k` | Desplazar 1 línea hacia abajo / arriba |
| `J` / `K` | Saltar al mensaje anterior / siguiente |
| `Ctrl+D` / `Ctrl+U` | Desplazar media página hacia abajo / arriba |
| `g` / `G` | Ir al principio / al final |
| `h` / `l` | Mover el cursor a la izquierda / derecha |
| `w` / `b` | Avanzar / retroceder una palabra |
| `0` / `$` | Principio / fin de línea |
| `x` | Eliminar el carácter bajo el cursor |
| `D` | Eliminar desde el cursor hasta el final |
| `y` / `Y` | Copiar el cuerpo del mensaje / la línea completa |
| `r` | Reaccionar al mensaje enfocado |
| `q` | Responder / citar el mensaje enfocado |
| `e` | Editar un mensaje propio enviado |
| `d` | Eliminar el mensaje (local o remoto) |
| `n` / `N` | Saltar al resultado de búsqueda siguiente / anterior |
| `i` | Entrar en modo Insert |
| `a` | Entrar en modo Insert (cursor 1 a la derecha) |
| `I` / `A` | Entrar en modo Insert al principio / final de la línea |
| `o` | Entrar en modo Insert (vaciar el búfer) |
| `/` | Entrar en modo Insert con `/` ya escrito |

### Modo Insert (predeterminado)

| Tecla | Acción |
|---|---|
| `Esc` | Cambiar a modo Normal |
| `Enter` | Enviar el mensaje / ejecutar el comando |
| `Shift+Enter` / `Alt+Enter` | Insertar un salto de línea (para mensajes de varias líneas) |
| `Backspace` / `Delete` | Eliminar caracteres |
| `Up` / `Down` | Recorrer el historial de entrada |
| `Left` / `Right` | Mover el cursor |
| `Home` / `End` | Ir al principio / fin de la línea |

## Arquitectura

```
Keyboard --> InputAction --> App state --> SignalClient (mpsc) --> signal-cli (JSON-RPC stdin/stdout)
signal-cli --> JsonRpcResponse --> SignalEvent (mpsc) --> App state --> SQLite + Ratatui render
```

```
+------------+   mpsc channels   +----------------+
|  TUI       | <---------------> |  Signal        |
|  (main     |   SignalEvent     |  Backend       |
|  thread)   |   UserCommand     |  (tokio task)  |
+------------+                   +--------+-------+
                                          |
                                   stdin/stdout
                                          |
                                 +--------v-------+
                                 |  signal-cli    |
                                 |  (child proc)  |
                                 +----------------+
```

Construido con [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) sobre el runtime asíncrono [Tokio](https://tokio.rs/).

## Licencia

[AGPL-3.0](../LICENSE)
