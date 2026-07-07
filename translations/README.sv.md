> This is the Swedish translation of the siggy README.
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
  <a href="README.es.md">Español</a>
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
  <b>Svenska</b>
  &nbsp;|&nbsp;
  <a href="README.ru.md">Русский</a>
  &nbsp;|&nbsp;
  <a href="README.uk.md">Українська</a>
  &nbsp;|&nbsp;
  <a href="README.zh-CN.md">简体中文</a>
  &nbsp;|&nbsp;
  <a href="../TRANSLATING.md">Bidra med en översättning</a>
</p>

En terminalbaserad klient för Signal med IRC-estetik. Använder [signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC som meddelandebackend.

![Skärmdump av siggy](../screenshot.png)

## Installation

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Färdigbyggda binärer

Ladda ner den senaste versionen för din plattform från [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (enradskommando):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Båda skripten laddar ner den senaste binären och kontrollerar att signal-cli finns.

### Från crates.io

Kräver Rust 1.70+.

```sh
cargo install siggy
```

### Bygg från källkod

Eller klona projektet och bygg lokalt:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Binären hamnar i target/release/siggy
```

## Förutsättningar

- [signal-cli](https://github.com/AsamK/signal-cli) installerat och tillgängligt via PATH (eller konfigurerat via `signal_cli_path`)
- Ett Signal-konto länkat som sekundär enhet (installationsguiden sköter detta)

## Användning

```sh
siggy                        # Starta (använder konfigurationsfilen)
siggy -a +15551234567        # Ange konto
siggy -c /path/to/config.toml  # Egen sökväg till konfigurationsfilen
siggy --setup                # Kör installationsguiden igen
siggy --demo                 # Starta med exempeldata (signal-cli behövs inte)
siggy --incognito            # Ingen lokal meddelandelagring (endast i minnet)
```

Vid första starten guidar installationsguiden dig genom att hitta signal-cli, ange ditt telefonnummer och länka din enhet via QR-kod.

## Konfiguration

Konfigurationen läses från:
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

Alla fält är valfria. `signal_cli_path` är som standard `"signal-cli"` (hittas via PATH) och `download_dir` är som standard `~/signal-downloads/`. På Windows anger du den fullständiga sökvägen till `signal-cli.bat` om den inte finns i PATH.

### Inline-bilder i tmux

Utanför tmux upptäcker siggy automatiskt Kitty / iTerm2 / WezTerm / Ghostty och renderar bilagor som riktiga pixelbilder. Inuti tmux krävs två inställningar, eftersom tmux döljer den yttre terminalen för siggy:

1. Säg åt tmux att vidarebefordra okända escape-sekvenser. Kräver tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Äldre tmux använder `set -g allow-passthrough all`.

2. Tala om för siggy vilket protokoll den yttre terminalen talar (autodetekteringen ser bara tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # eller iterm2 / sixel / halfblock
   ```

Om `SIGGY_IMAGE_PROTOCOL` inte är satt körs den vanliga autodetekteringen (korrekt utanför tmux, faller tillbaka till halfblock inuti tmux). Sixel passerar genom tmux 3.4+ utan vidare och behöver inte miljövariabeln.

## Funktioner

- **Meddelanden** -- Skicka och ta emot direktmeddelanden och gruppmeddelanden
- **Bilagor** -- Bildförhandsvisningar renderas inline som halfblock-konst; andra bilagor visas som `[attachment: filnamn]`
- **Klickbara länkar** -- URL:er och filsökvägar är OSC 8-hyperlänkar (klickbara i terminaler som Windows Terminal, iTerm2 med flera)
- **Skrivindikatorer** -- Visar vem som skriver, med uppslagning av kontaktnamn
- **Meddelandesynkning** -- Meddelanden som skickas från din telefon dyker upp i TUI:t
- **Beständig lagring** -- Meddelanden lagras i SQLite med WAL-läge; konversationer och läsmarkörer överlever omstarter
- **Olästa meddelanden** -- Antal olästa visas i sidofältet, med en "nya meddelanden"-avdelare i chatten
- **Aviseringar** -- Terminalklocka vid nya meddelanden (konfigurerbar för direkt-/gruppchattar, går att tysta per chatt) samt skrivbordsaviseringar på OS-nivå
- **Kontaktnamn** -- Namn hämtas från din Signal-adressbok; grupper fylls i automatiskt vid start
- **Reaktioner** -- Reagera med `r` i Normal-läge; emojiväljare med badge-visning (`👍 2 ❤️ 1`)
- **Svara / citera** -- Tryck `q` på ett fokuserat meddelande för att svara med citerat sammanhang
- **Redigera meddelanden** -- Tryck `e` för att redigera dina egna skickade meddelanden
- **Radera meddelanden** -- Tryck `d` för att radera lokalt eller hos mottagaren (för dina egna meddelanden)
- **Radera konversationer** -- Använd `/delete` för att ta bort den aktuella konversationen lokalt (avböjer väntande meddelandeförfrågningar)
- **Meddelandesökning** -- `/search <sökterm>` med `n`/`N` för att hoppa mellan träffarna
- **@omnämnanden** -- Skriv `@` i gruppchattar för att nämna medlemmar med autokomplettering
- **Meddelandeval** -- Fokuserat meddelande markeras vid scrollning; `J`/`K` hoppar mellan meddelanden
- **Läskvitton** -- Statussymboler på utgående meddelanden (Skickar → Skickat → Levererat → Läst → Visat)
- **Försvinnande meddelanden** -- Respekterar Signals timer för försvinnande meddelanden; ställs in per konversation med `/disappearing`
- **Grupphantering** -- Skapa grupper, lägg till/ta bort medlemmar, byt namn eller lämna gruppen via `/group`
- **Meddelandeförfrågningar** -- Acceptera eller radera meddelanden från okända avsändare
- **Blockera / avblockera** -- Blockera kontakter eller grupper med `/block` och `/unblock`
- **Musstöd** -- Klicka på konversationer i sidofältet, scrolla bland meddelanden, klicka för att placera markören
- **Färgteman** -- Valbara teman via `/theme` eller `/settings`
- **Installationsguide** -- Introduktion vid första start med enhetslänkning via QR-kod
- **Vim-tangentbindningar** -- Modal redigering (Normal/Insert) med fullständig markörnavigering
- **Autokomplettering av kommandon** -- Popup med Tab-komplettering för slash-kommandon
- **Inställningsoverlay** -- Slå på och av aviseringar, sidofält och inline-bilder direkt i appen
- **Responsiv layout** -- Sidofältet kan storleksändras och döljs automatiskt i smala terminaler (<60 kolumner)
- **Inkognitoläge** -- `--incognito` lagrar allt i minnet; inget sparas efter avslut
- **Proxystöd** -- Konfigurera en TLS-proxy för Signal via konfigurationsfältet `proxy`, för användning i begränsade nätverk
- **Demoläge** -- Prova gränssnittet utan signal-cli (`--demo`)

## Kommandon

| Kommando | Alias | Beskrivning |
|---|---|---|
| `/join <namn>` | `/j` | Byt till en konversation via kontaktnamn, nummer eller grupp |
| `/part` | `/p` | Lämna den aktuella konversationen |
| `/delete` | | Radera den aktuella konversationen (avböjer väntande meddelandeförfrågningar) |
| `/attach` | `/a` | Öppna filbläddraren för att bifoga en fil |
| `/search <sökterm>` | `/s` | Sök meddelanden i den aktuella konversationen (eller alla) |
| `/sidebar` | `/sb` | Visa/dölj sidofältet |
| `/bell [typ]` | `/notify` | Slå på/av aviseringar (`direct`, `group` eller båda) |
| `/mute [tid]` | | Tysta/avtysta den aktuella konversationen (t.ex. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Blockera den aktuella kontakten eller gruppen |
| `/unblock` | | Avblockera den aktuella kontakten eller gruppen |
| `/disappearing <tid>` | `/dm` | Ställ in timer för försvinnande meddelanden (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Öppna menyn för grupphantering |
| `/theme` | `/t` | Öppna temaväljaren |
| `/contacts` | `/c` | Bläddra bland synkade kontakter |
| `/settings` | | Öppna inställningsoverlayen |
| `/lock` | | Lås sessionen |
| `/lock-reset` | | Byt lösenfras för låset (kräver den nuvarande lösenfrasen) |
| `/help` | `/h` | Visa hjälpoverlayen |
| `/quit` | `/q` | Avsluta siggy |

Skriv `/` för att öppna autokompletteringspopupen. Använd `Tab` för att komplettera och piltangenterna för att navigera.

För att skriva till en ny kontakt: `/join +15551234567` (E.164-format).

**Har du glömt lösenfrasen till låset?** Avsluta siggy (eller döda processen) och kör `siggy --reset-lock`. Kommandot raderar den sparade lösenfrashashen och skriver ut sökvägen som togs bort. Nästa `/lock` sätter en ny lösenfras.

## Tangentbordsgenvägar

Appen använder modal redigering i vim-stil med två lägen: **Insert** (standard) och **Normal**.

### Globala (båda lägena)

| Tangent | Funktion |
|---|---|
| `Ctrl+C` | Avsluta |
| `Tab` / `Shift+Tab` | Nästa / föregående konversation |
| `PgUp` / `PgDn` | Scrolla meddelanden (5 rader) |
| `Ctrl+Left` / `Ctrl+Right` | Ändra sidofältets bredd |

### Normal-läge

Tryck `Esc` för att gå till Normal-läge.

| Tangent | Funktion |
|---|---|
| `j` / `k` | Scrolla ner / upp 1 rad |
| `J` / `K` | Hoppa till föregående / nästa meddelande |
| `Ctrl+D` / `Ctrl+U` | Scrolla ner / upp en halv sida |
| `g` / `G` | Scrolla till början / slutet |
| `h` / `l` | Flytta markören åt vänster / höger |
| `w` / `b` | Ett ord framåt / bakåt |
| `0` / `$` | Början / slutet av raden |
| `x` | Radera tecknet vid markören |
| `D` | Radera från markören till radens slut |
| `y` / `Y` | Kopiera meddelandetexten / hela raden |
| `r` | Reagera på fokuserat meddelande |
| `q` | Svara på / citera fokuserat meddelande |
| `e` | Redigera eget skickat meddelande |
| `d` | Radera meddelande (lokalt eller hos mottagaren) |
| `n` / `N` | Hoppa till nästa / föregående sökträff |
| `i` | Gå till Insert-läge |
| `a` | Gå till Insert-läge (markören ett steg åt höger) |
| `I` / `A` | Gå till Insert-läge i början / slutet av raden |
| `o` | Gå till Insert-läge (töm bufferten) |
| `/` | Gå till Insert-läge med `/` förifyllt |

### Insert-läge (standard)

| Tangent | Funktion |
|---|---|
| `Esc` | Växla till Normal-läge |
| `Enter` | Skicka meddelande / kör kommando |
| `Shift+Enter` / `Alt+Enter` | Infoga radbrytning (för flerradiga meddelanden) |
| `Backspace` / `Delete` | Radera tecken |
| `Up` / `Down` | Bläddra i inmatningshistoriken |
| `Left` / `Right` | Flytta markören |
| `Home` / `End` | Hoppa till början / slutet av raden |

## Arkitektur

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

Byggd med [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) ovanpå den asynkrona körmiljön [Tokio](https://tokio.rs/).

## Licens

[AGPL-3.0](../LICENSE)
