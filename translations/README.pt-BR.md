> This is the Brazilian Portuguese translation of the siggy README.
> Last updated against English commit: 8b5890e
> The [English version](../README.md) is authoritative. If this translation has drifted, trust the English.
> This translation is maintainer-provided and awaiting native-speaker review. Corrections welcome - see issue #353.

<p align="center">
  <img src="../siggy-banner.png" alt="siggy" width="600">
</p>

<p align="center">
  <a href="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml"><img src="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/johnsideserf/siggy/releases/latest"><img src="https://img.shields.io/github/v/release/johnsideserf/siggy" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/johnsideserf/siggy" alt="License: GPL-3.0"></a>
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
  <a href="README.es.md">Español</a>
  &nbsp;|&nbsp;
  <a href="README.fr.md">Français</a>
  &nbsp;|&nbsp;
  <a href="README.it.md">Italiano</a>
  &nbsp;|&nbsp;
  <a href="README.nl.md">Nederlands</a>
  &nbsp;|&nbsp;
  <b>Português</b>
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
  <a href="../TRANSLATING.md">Contribua com uma tradução</a>
</p>

Um cliente do Signal para terminal com estética de IRC. Encapsula o [signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC como backend de mensagens.

![captura de tela do siggy](../screenshot.png)

## Instalação

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Binários pré-compilados

Baixe a versão mais recente para a sua plataforma na página de [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (comando único):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Os dois scripts baixam o binário da versão mais recente e verificam a presença do signal-cli.

### Pelo crates.io

Requer Rust 1.70+.

```sh
cargo install siggy
```

### Compilar a partir do código-fonte

Ou clone o repositório e compile localmente:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# O binário fica em target/release/siggy
```

## Pré-requisitos

- [signal-cli](https://github.com/AsamK/signal-cli) instalado e acessível pelo PATH (ou configurado via `signal_cli_path`)
- Uma conta do Signal vinculada como dispositivo secundário (o assistente de configuração cuida disso)

## Uso

```sh
siggy                        # Inicia (usa o arquivo de configuração)
siggy -a +15551234567        # Especifica a conta
siggy -c /path/to/config.toml  # Caminho de configuração personalizado
siggy --setup                # Executa novamente o assistente de configuração inicial
siggy --demo                 # Inicia com dados fictícios (não precisa do signal-cli)
siggy --incognito            # Sem armazenamento local de mensagens (somente em memória)
```

Na primeira execução, o assistente de configuração orienta você a localizar o signal-cli, informar seu número de telefone e vincular seu dispositivo via QR code.

## Configuração

A configuração é carregada de:
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

Todos os campos são opcionais. O valor padrão de `signal_cli_path` é `"signal-cli"` (localizado pelo PATH), e o de `download_dir` é `~/signal-downloads/`. No Windows, use o caminho completo para `signal-cli.bat` caso ele não esteja no seu PATH.

### Imagens inline dentro do tmux

Fora do tmux, o siggy detecta automaticamente Kitty / iTerm2 / WezTerm / Ghostty e renderiza os anexos como imagens nativas em pixels. Dentro do tmux, é preciso configurar duas coisas, porque o tmux esconde o terminal externo do siggy:

1. Configure o tmux para repassar sequências de escape desconhecidas. Requer tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Em versões mais antigas do tmux, use `set -g allow-passthrough all`.

2. Informe ao siggy qual protocolo o terminal externo usa (a detecção automática enxerga apenas o tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # ou iterm2 / sixel / halfblock
   ```

Se `SIGGY_IMAGE_PROTOCOL` não estiver definida, a detecção automática padrão entra em ação (funciona corretamente fora do tmux e recorre a halfblock dentro dele). O sixel atravessa o tmux 3.4+ nativamente e dispensa a variável de ambiente.

## Funcionalidades

- **Mensagens** -- Envie e receba mensagens individuais e de grupo
- **Anexos** -- Pré-visualização de imagens renderizada inline como arte em halfblock; anexos que não são imagens aparecem como `[attachment: nome_do_arquivo]`
- **Links clicáveis** -- URLs e caminhos de arquivo são hyperlinks OSC 8 (clicáveis em terminais como Windows Terminal, iTerm2 etc.)
- **Indicadores de digitação** -- Mostra quem está digitando, com resolução do nome do contato
- **Sincronização de mensagens** -- Mensagens enviadas pelo seu celular aparecem na TUI
- **Persistência** -- Armazenamento de mensagens em SQLite com modo WAL; conversas e marcadores de leitura sobrevivem a reinicializações
- **Controle de não lidas** -- Contadores de mensagens não lidas na barra lateral, com separador de "novas mensagens" no chat
- **Notificações** -- Sino do terminal ao receber novas mensagens (configurável para conversas diretas/grupos, com silenciamento por conversa) e notificações de desktop do sistema operacional
- **Resolução de contatos** -- Nomes vindos da sua agenda do Signal; grupos preenchidos automaticamente na inicialização
- **Reações a mensagens** -- Reaja com `r` no modo Normal; seletor de emojis com exibição de badges (`👍 2 ❤️ 1`)
- **Responder / citar** -- Pressione `q` em uma mensagem em foco para responder citando o contexto
- **Editar mensagens** -- Pressione `e` para editar suas próprias mensagens enviadas
- **Apagar mensagens** -- Pressione `d` para apagar localmente ou remotamente (no caso das suas próprias mensagens)
- **Apagar conversas** -- Use `/delete` para remover a conversa atual localmente (recusa solicitações de mensagem pendentes)
- **Busca de mensagens** -- `/search <termo>` com `n`/`N` para pular entre os resultados
- **@menções** -- Digite `@` em chats de grupo para mencionar membros com autocompletar
- **Seleção de mensagens** -- Destaque da mensagem em foco durante a rolagem; `J`/`K` para pular entre mensagens
- **Confirmações de leitura** -- Símbolos de status nas mensagens enviadas (Enviando → Enviada → Entregue → Lida → Visualizada)
- **Mensagens temporárias** -- Respeita os temporizadores de mensagens temporárias do Signal; configure por conversa com `/disappearing`
- **Gerenciamento de grupos** -- Crie grupos, adicione/remova membros, renomeie e saia via `/group`
- **Solicitações de mensagem** -- Aceite ou apague mensagens de remetentes desconhecidos
- **Bloquear / desbloquear** -- Bloqueie contatos ou grupos com `/block` e `/unblock`
- **Suporte a mouse** -- Clique nas conversas da barra lateral, role as mensagens, clique para posicionar o cursor
- **Temas de cores** -- Temas selecionáveis via `/theme` ou `/settings`
- **Assistente de configuração** -- Integração na primeira execução com vinculação de dispositivo via QR code
- **Atalhos do Vim** -- Edição modal (Normal/Insert) com movimentação completa do cursor
- **Autocompletar de comandos** -- Popup de conclusão via Tab para slash commands
- **Overlay de configurações** -- Ative/desative notificações, barra lateral e imagens inline de dentro do app
- **Layout responsivo** -- Barra lateral redimensionável que se oculta automaticamente em terminais estreitos (<60 colunas)
- **Modo anônimo** -- `--incognito` usa armazenamento em memória; nada é mantido após sair
- **Suporte a proxy** -- Configure um proxy TLS do Signal pelo campo de configuração `proxy` para uso em redes restritas
- **Modo demo** -- Experimente a interface sem o signal-cli (`--demo`)

## Comandos

| Comando | Alias | Descrição |
|---|---|---|
| `/join <nome>` | `/j` | Alternar para uma conversa por nome de contato, número ou grupo |
| `/part` | `/p` | Sair da conversa atual |
| `/delete` | | Apagar a conversa atual (recusa solicitações de mensagem pendentes) |
| `/attach` | `/a` | Abrir o navegador de arquivos para anexar um arquivo |
| `/search <termo>` | `/s` | Buscar mensagens na conversa atual (ou em todas) |
| `/sidebar` | `/sb` | Mostrar/ocultar a barra lateral |
| `/bell [tipo]` | `/notify` | Ativar/desativar notificações (`direct`, `group` ou ambas) |
| `/mute [duração]` | | Silenciar/reativar a conversa atual (ex.: `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Bloquear o contato ou grupo atual |
| `/unblock` | | Desbloquear o contato ou grupo atual |
| `/disappearing <dur>` | `/dm` | Definir o temporizador de mensagens temporárias (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Abrir o menu de gerenciamento de grupos |
| `/theme` | `/t` | Abrir o seletor de temas |
| `/contacts` | `/c` | Navegar pelos contatos sincronizados |
| `/settings` | | Abrir o overlay de configurações |
| `/lock` | | Bloquear a sessão |
| `/lock-reset` | | Alterar a senha de bloqueio (requer a senha atual) |
| `/help` | `/h` | Mostrar o overlay de ajuda |
| `/quit` | `/q` | Sair do siggy |

Digite `/` para abrir o popup de autocompletar. Use `Tab` para completar e as setas para navegar.

Para enviar mensagem a um novo contato: `/join +15551234567` (formato E.164).

**Esqueceu a senha de bloqueio?** Saia do siggy (ou encerre o processo) e execute `siggy --reset-lock`. O comando apaga o hash da senha armazenada e exibe o caminho do arquivo removido. O próximo `/lock` definirá uma nova senha.

## Atalhos de teclado

O app usa edição modal no estilo Vim com dois modos: **Insert** (padrão) e **Normal**.

### Globais (ambos os modos)

| Tecla | Ação |
|---|---|
| `Ctrl+C` | Sair |
| `Tab` / `Shift+Tab` | Conversa seguinte / anterior |
| `PgUp` / `PgDn` | Rolar as mensagens (5 linhas) |
| `Ctrl+Left` / `Ctrl+Right` | Redimensionar a barra lateral |

### Modo Normal

Pressione `Esc` para entrar no modo Normal.

| Tecla | Ação |
|---|---|
| `j` / `k` | Rolar 1 linha para baixo / para cima |
| `J` / `K` | Pular para a mensagem anterior / seguinte |
| `Ctrl+D` / `Ctrl+U` | Rolar meia página para baixo / para cima |
| `g` / `G` | Rolar até o topo / o fim |
| `h` / `l` | Mover o cursor para a esquerda / direita |
| `w` / `b` | Avançar / recuar uma palavra |
| `0` / `$` | Início / fim da linha |
| `x` | Apagar o caractere sob o cursor |
| `D` | Apagar do cursor até o fim |
| `y` / `Y` | Copiar o corpo da mensagem / a linha inteira |
| `r` | Reagir à mensagem em foco |
| `q` | Responder / citar a mensagem em foco |
| `e` | Editar a própria mensagem enviada |
| `d` | Apagar a mensagem (local ou remotamente) |
| `n` / `N` | Pular para o resultado seguinte / anterior da busca |
| `i` | Entrar no modo Insert |
| `a` | Entrar no modo Insert (cursor 1 à direita) |
| `I` / `A` | Entrar no modo Insert no início / fim da linha |
| `o` | Entrar no modo Insert (limpar o buffer) |
| `/` | Entrar no modo Insert com `/` já digitado |

### Modo Insert (padrão)

| Tecla | Ação |
|---|---|
| `Esc` | Mudar para o modo Normal |
| `Enter` | Enviar mensagem / executar comando |
| `Shift+Enter` / `Alt+Enter` | Inserir quebra de linha (para mensagens com várias linhas) |
| `Backspace` / `Delete` | Apagar caracteres |
| `Up` / `Down` | Recuperar o histórico de entrada |
| `Left` / `Right` | Mover o cursor |
| `Home` / `End` | Pular para o início / fim da linha |

## Arquitetura

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

Construído com [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) sobre o runtime assíncrono [Tokio](https://tokio.rs/).

## Licença

[GPL-3.0](../LICENSE)
