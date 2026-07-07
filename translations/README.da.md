> This is the Danish translation of the siggy README.
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
  <b>Dansk</b>
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
  <a href="README.sv.md">Svenska</a>
  &nbsp;|&nbsp;
  <a href="README.ru.md">Русский</a>
  &nbsp;|&nbsp;
  <a href="README.uk.md">Українська</a>
  &nbsp;|&nbsp;
  <a href="README.zh-CN.md">简体中文</a>
  &nbsp;|&nbsp;
  <a href="../TRANSLATING.md">Bidrag med en oversættelse</a>
</p>

En terminalbaseret Signal-klient med IRC-æstetik. Bruger [signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC som beskedbackend.

![siggy skærmbillede](../screenshot.png)

## Installation

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Forudbyggede binærfiler

Hent den seneste udgivelse til din platform fra [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (one-liner):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Begge scripts henter den seneste binærfil og tjekker, om signal-cli er installeret.

### Fra crates.io

Kræver Rust 1.70+.

```sh
cargo install siggy
```

### Byg fra kildekode

Eller klon projektet og byg det selv:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Binærfilen ligger i target/release/siggy
```

## Forudsætninger

- [signal-cli](https://github.com/AsamK/signal-cli) installeret og tilgængelig via PATH (eller angivet via `signal_cli_path`)
- En Signal-konto tilknyttet som sekundær enhed (opsætningsguiden klarer dette)

## Brug

```sh
siggy                        # Start (bruger konfigurationsfilen)
siggy -a +15551234567        # Angiv konto
siggy -c /path/to/config.toml  # Brugerdefineret konfigurationssti
siggy --setup                # Kør førstegangsopsætningen igen
siggy --demo                 # Start med testdata (kræver ikke signal-cli)
siggy --incognito            # Ingen lokal lagring af beskeder (kun i hukommelsen)
```

Ved første start hjælper opsætningsguiden dig med at finde signal-cli, indtaste dit telefonnummer og tilknytte din enhed via en QR-kode.

## Konfiguration

Konfigurationen indlæses fra:
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

Alle felter er valgfrie. `signal_cli_path` er som standard `"signal-cli"` (findes via PATH), og `download_dir` er som standard `~/signal-downloads/`. På Windows skal du angive den fulde sti til `signal-cli.bat`, hvis den ikke ligger i din PATH.

### Inline-billeder i tmux

Uden for tmux registrerer siggy automatisk Kitty / iTerm2 / WezTerm / Ghostty og viser vedhæftede filer som rigtige pixelbilleder. Inde i tmux skal to ting sættes op, fordi tmux skjuler den ydre terminal for siggy:

1. Bed tmux om at videresende ukendte escape-sekvenser. Kræver tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Ældre tmux-versioner bruger `set -g allow-passthrough all`.

2. Fortæl siggy, hvilken protokol den ydre terminal taler (autodetekteringen ser kun tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # eller iterm2 / sixel / halfblock
   ```

Hvis `SIGGY_IMAGE_PROTOCOL` ikke er sat, kører den almindelige autodetektering (korrekt uden for tmux, falder tilbage til halfblock indeni). Sixel passerer gennem tmux 3.4+ uden videre og behøver ikke miljøvariablen.

## Funktioner

- **Beskeder** -- Send og modtag både 1:1- og gruppebeskeder
- **Vedhæftede filer** -- Billeder forhåndsvises inline som halfblock-grafik; andre vedhæftede filer vises som `[attachment: filnavn]`
- **Klikbare links** -- URL'er og filstier er OSC 8-hyperlinks (klikbare i terminaler som Windows Terminal, iTerm2 m.fl.)
- **Skriveindikatorer** -- Viser, hvem der er ved at skrive, med opslag af kontaktnavn
- **Beskedsynkronisering** -- Beskeder sendt fra din telefon dukker op i TUI'en
- **Persistens** -- Beskederne gemmes i SQLite med WAL-tilstand; samtaler og læsemarkører overlever genstart
- **Ulæste beskeder** -- Antal ulæste vises i sidepanelet med en "nye beskeder"-skillelinje i chatten
- **Notifikationer** -- Terminalklokke ved nye beskeder (kan indstilles separat for direkte/gruppe og slås fra pr. samtale) samt skrivebordsnotifikationer på OS-niveau
- **Kontaktopslag** -- Navne fra din Signal-adressebog; grupper indlæses automatisk ved opstart
- **Reaktioner** -- Reagér med `r` i Normal-tilstand; emoji-vælger med badgevisning (`👍 2 ❤️ 1`)
- **Svar / citat** -- Tryk på `q` på en fokuseret besked for at svare med citeret kontekst
- **Redigér beskeder** -- Tryk på `e` for at redigere dine egne sendte beskeder
- **Slet beskeder** -- Tryk på `d` for at slette lokalt eller hos modtageren (for dine egne beskeder)
- **Slet samtaler** -- Brug `/delete` til at fjerne den aktuelle samtale lokalt (afviser ventende beskedanmodninger)
- **Beskedsøgning** -- `/search <søgeord>` med `n`/`N` til at hoppe mellem resultaterne
- **@mentions** -- Skriv `@` i gruppechats for at nævne medlemmer med autofuldførelse
- **Beskedvalg** -- Den fokuserede besked fremhæves under scroll; `J`/`K` hopper mellem beskeder
- **Læsekvitteringer** -- Statussymboler på udgående beskeder (Sender → Sendt → Leveret → Læst → Set)
- **Forsvindende beskeder** -- Respekterer Signals tidsgrænser for forsvindende beskeder; indstilles pr. samtale med `/disappearing`
- **Gruppestyring** -- Opret grupper, tilføj/fjern medlemmer, omdøb og forlad grupper via `/group`
- **Beskedanmodninger** -- Acceptér eller slet beskeder fra ukendte afsendere
- **Blokér / fjern blokering** -- Blokér kontakter eller grupper med `/block` og `/unblock`
- **Musestøtte** -- Klik på samtaler i sidepanelet, scroll i beskederne, klik for at placere markøren
- **Farvetemaer** -- Vælg tema via `/theme` eller `/settings`
- **Opsætningsguide** -- Førstegangsopsætning med enhedstilknytning via QR-kode
- **Vim-tastebindinger** -- Modal redigering (Normal/Insert) med fuld markørnavigation
- **Autofuldførelse af kommandoer** -- Tab-fuldførelse i en popup for slash-kommandoer
- **Indstillingsoverlay** -- Slå notifikationer, sidepanel og inline-billeder til og fra inde fra appen
- **Responsivt layout** -- Sidepanel, der kan ændres i størrelse og automatisk skjules på smalle terminaler (<60 kolonner)
- **Inkognitotilstand** -- `--incognito` gemmer kun i hukommelsen; intet bevares efter afslutning
- **Proxy-understøttelse** -- Konfigurér en Signal TLS-proxy via konfigurationsfeltet `proxy` til brug på begrænsede netværk
- **Demotilstand** -- Prøv brugerfladen uden signal-cli (`--demo`)

## Kommandoer

| Kommando | Alias | Beskrivelse |
|---|---|---|
| `/join <name>` | `/j` | Skift til en samtale via kontaktnavn, nummer eller gruppe |
| `/part` | `/p` | Forlad den aktuelle samtale |
| `/delete` | | Slet den aktuelle samtale (afviser ventende beskedanmodninger) |
| `/attach` | `/a` | Åbn filbrowseren for at vedhæfte en fil |
| `/search <query>` | `/s` | Søg i beskeder i den aktuelle samtale (eller alle samtaler) |
| `/sidebar` | `/sb` | Vis/skjul sidepanelet |
| `/bell [type]` | `/notify` | Slå notifikationer til/fra (`direct`, `group` eller begge) |
| `/mute [duration]` | | Slå lyden fra/til for den aktuelle samtale (f.eks. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Blokér den aktuelle kontakt eller gruppe |
| `/unblock` | | Fjern blokeringen af den aktuelle kontakt eller gruppe |
| `/disappearing <dur>` | `/dm` | Indstil timer for forsvindende beskeder (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Åbn menuen for gruppestyring |
| `/theme` | `/t` | Åbn temavælgeren |
| `/contacts` | `/c` | Gennemse synkroniserede kontakter |
| `/settings` | | Åbn indstillingsoverlayet |
| `/lock` | | Lås sessionen |
| `/lock-reset` | | Skift låsekodeordet (kræver det nuværende kodeord) |
| `/help` | `/h` | Vis hjælpeoverlayet |
| `/quit` | `/q` | Afslut siggy |

Skriv `/` for at åbne autofuldførelses-popuppen. Brug `Tab` til at fuldføre og piletasterne til at navigere.

Sådan skriver du til en ny kontakt: `/join +15551234567` (E.164-format).

**Har du glemt dit låsekodeord?** Afslut siggy (eller dræb processen) og kør `siggy --reset-lock`. Kommandoen sletter den gemte kodeords-hash og udskriver stien, der blev fjernet. Næste `/lock` sætter et nyt kodeord.

## Tastaturgenveje

Appen bruger modal redigering i vim-stil med to tilstande: **Insert** (standard) og **Normal**.

### Globalt (begge tilstande)

| Tast | Handling |
|---|---|
| `Ctrl+C` | Afslut |
| `Tab` / `Shift+Tab` | Næste / forrige samtale |
| `PgUp` / `PgDn` | Scroll i beskederne (5 linjer) |
| `Ctrl+Left` / `Ctrl+Right` | Skift størrelse på sidepanelet |

### Normal-tilstand

Tryk på `Esc` for at skifte til Normal-tilstand.

| Tast | Handling |
|---|---|
| `j` / `k` | Scroll ned / op 1 linje |
| `J` / `K` | Hop til forrige / næste besked |
| `Ctrl+D` / `Ctrl+U` | Scroll ned / op en halv side |
| `g` / `G` | Scroll til top / bund |
| `h` / `l` | Flyt markøren til venstre / højre |
| `w` / `b` | Et ord frem / tilbage |
| `0` / `$` | Start / slutning af linjen |
| `x` | Slet tegnet under markøren |
| `D` | Slet fra markøren til slutningen |
| `y` / `Y` | Kopiér beskedteksten / hele linjen |
| `r` | Reagér på den fokuserede besked |
| `q` | Svar på / citér den fokuserede besked |
| `e` | Redigér din egen sendte besked |
| `d` | Slet besked (lokalt eller hos modtageren) |
| `n` / `N` | Hop til næste / forrige søgeresultat |
| `i` | Skift til Insert-tilstand |
| `a` | Skift til Insert-tilstand (markøren 1 til højre) |
| `I` / `A` | Skift til Insert-tilstand ved linjens start / slutning |
| `o` | Skift til Insert-tilstand (ryd bufferen) |
| `/` | Skift til Insert-tilstand med `/` allerede indtastet |

### Insert-tilstand (standard)

| Tast | Handling |
|---|---|
| `Esc` | Skift til Normal-tilstand |
| `Enter` | Send besked / udfør kommando |
| `Shift+Enter` / `Alt+Enter` | Indsæt linjeskift (til beskeder over flere linjer) |
| `Backspace` / `Delete` | Slet tegn |
| `Up` / `Down` | Bladr i inputhistorikken |
| `Left` / `Right` | Flyt markøren |
| `Home` / `End` | Hop til linjens start / slutning |

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

Bygget med [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) oven på den asynkrone [Tokio](https://tokio.rs/)-runtime.

## Licens

[AGPL-3.0](../LICENSE)
