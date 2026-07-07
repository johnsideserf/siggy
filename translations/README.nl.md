> This is the Dutch translation of the siggy README.
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
  <b>Nederlands</b>
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
  <a href="../TRANSLATING.md">Draag een vertaling bij</a>
</p>

Een Signal-messengerclient voor de terminal, met een IRC-uitstraling. Gebruikt [signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC als messaging-backend.

![schermafbeelding van siggy](../screenshot.png)

## Installatie

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Voorgecompileerde binaries

Download de nieuwste release voor jouw platform via [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (oneliner):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Beide scripts downloaden de nieuwste release-binary en controleren of signal-cli aanwezig is.

### Via crates.io

Vereist Rust 1.70+.

```sh
cargo install siggy
```

### Bouwen vanuit de broncode

Of kloon de repository en bouw lokaal:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# De binary staat in target/release/siggy
```

## Vereisten

- [signal-cli](https://github.com/AsamK/signal-cli) geïnstalleerd en bereikbaar via PATH (of ingesteld via `signal_cli_path`)
- Een Signal-account dat als secundair apparaat is gekoppeld (de installatiewizard regelt dit)

## Gebruik

```sh
siggy                        # Starten (gebruikt het configuratiebestand)
siggy -a +15551234567        # Account opgeven
siggy -c /path/to/config.toml  # Eigen pad naar het configuratiebestand
siggy --setup                # Installatiewizard opnieuw uitvoeren
siggy --demo                 # Starten met voorbeeldgegevens (geen signal-cli nodig)
siggy --incognito            # Geen lokale berichtopslag (alleen in het geheugen)
```

Bij de eerste start leidt de installatiewizard je door het vinden van signal-cli, het invoeren van je telefoonnummer en het koppelen van je apparaat via een QR-code.

## Configuratie

De configuratie wordt geladen vanuit:
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

Alle velden zijn optioneel. `signal_cli_path` is standaard `"signal-cli"` (gevonden via PATH) en `download_dir` is standaard `~/signal-downloads/`. Gebruik op Windows het volledige pad naar `signal-cli.bat` als die niet in je PATH staat.

### Inline afbeeldingen binnen tmux

Buiten tmux detecteert siggy automatisch Kitty / iTerm2 / WezTerm / Ghostty en worden bijlagen als native pixelafbeeldingen weergegeven. Binnen tmux moet je twee dingen instellen, omdat tmux de buitenste terminal voor siggy verbergt:

1. Laat tmux onbekende escape-sequenties doorgeven. Vereist tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Oudere tmux-versies gebruiken `set -g allow-passthrough all`.

2. Vertel siggy welk protocol de buitenste terminal spreekt (de autodetectie ziet alleen tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # of iterm2 / sixel / halfblock
   ```

Als `SIGGY_IMAGE_PROTOCOL` niet is ingesteld, draait de gewone autodetectie (correct buiten tmux, valt binnen tmux terug op halfblock). Sixel wordt vanaf tmux 3.4+ native doorgegeven en heeft de omgevingsvariabele niet nodig.

## Functies

- **Berichten** -- Verstuur en ontvang 1-op-1- en groepsberichten
- **Bijlagen** -- Voorbeelden van afbeeldingen worden inline weergegeven als halfblok-art; andere bijlagen verschijnen als `[attachment: bestandsnaam]`
- **Klikbare links** -- URL's en bestandspaden zijn OSC 8-hyperlinks (klikbaar in terminals zoals Windows Terminal, iTerm2, enz.)
- **Typindicatoren** -- Laat zien wie er aan het typen is, met weergave van de contactnaam
- **Berichtsynchronisatie** -- Berichten die je vanaf je telefoon verstuurt, verschijnen in de TUI
- **Persistentie** -- Berichtopslag in SQLite met WAL-modus; gesprekken en leesmarkeringen blijven bewaard na een herstart
- **Ongelezen berichten** -- Tellers voor ongelezen berichten in de zijbalk, met een "nieuwe berichten"-scheidingslijn in de chat
- **Meldingen** -- Terminalbel bij nieuwe berichten (apart instelbaar voor directe en groepsgesprekken, dempen per gesprek) en bureaubladmeldingen via het besturingssysteem
- **Contactnamen** -- Namen uit je Signal-adresboek; groepen worden bij het opstarten automatisch geladen
- **Reacties op berichten** -- Reageer met `r` in de Normal-modus; emojikiezer met badgeweergave (`👍 2 ❤️ 1`)
- **Beantwoorden / citeren** -- Druk op `q` bij een geselecteerd bericht om te antwoorden met geciteerde context
- **Berichten bewerken** -- Druk op `e` om je eigen verzonden berichten te bewerken
- **Berichten verwijderen** -- Druk op `d` om lokaal of op afstand te verwijderen (voor je eigen berichten)
- **Gesprekken verwijderen** -- Gebruik `/delete` om het huidige gesprek lokaal te verwijderen (wijst openstaande berichtverzoeken af)
- **Berichten zoeken** -- `/search <zoekterm>` met `n`/`N` om tussen de resultaten te springen
- **@vermeldingen** -- Typ `@` in groepsgesprekken om leden te vermelden, met autocomplete
- **Berichtselectie** -- Het geselecteerde bericht wordt gemarkeerd tijdens het scrollen; spring met `J`/`K` van bericht naar bericht
- **Leesbevestigingen** -- Statussymbolen bij uitgaande berichten (Verzenden → Verzonden → Afgeleverd → Gelezen → Bekeken)
- **Verdwijnende berichten** -- Respecteert de timers voor verdwijnende berichten van Signal; per gesprek in te stellen met `/disappearing`
- **Groepsbeheer** -- Groepen aanmaken, leden toevoegen of verwijderen, hernoemen en verlaten via `/group`
- **Berichtverzoeken** -- Accepteer of verwijder berichten van onbekende afzenders
- **Blokkeren / deblokkeren** -- Blokkeer contacten of groepen met `/block` en `/unblock`
- **Muisondersteuning** -- Klik op gesprekken in de zijbalk, scroll door berichten, klik om de cursor te plaatsen
- **Kleurthema's** -- Kies een thema via `/theme` of `/settings`
- **Installatiewizard** -- Begeleide eerste configuratie met apparaatkoppeling via QR-code
- **Vim-toetsbindingen** -- Modaal bewerken (Normal/Insert) met volledige cursorbesturing
- **Autocomplete voor commando's** -- Tab-completion-popup voor slash-commando's
- **Instellingenoverlay** -- Schakel meldingen, de zijbalk en inline afbeeldingen in of uit vanuit de app
- **Responsieve indeling** -- Verstelbare zijbalk die zich automatisch verbergt op smalle terminals (<60 kolommen)
- **Incognitomodus** -- `--incognito` gebruikt opslag in het geheugen; na afsluiten blijft er niets bewaard
- **Proxyondersteuning** -- Stel via het configuratieveld `proxy` een Signal TLS-proxy in voor gebruik op beperkte netwerken
- **Demomodus** -- Probeer de interface zonder signal-cli (`--demo`)

## Commando's

| Commando | Alias | Beschrijving |
|---|---|---|
| `/join <naam>` | `/j` | Ga naar een gesprek op contactnaam, nummer of groep |
| `/part` | `/p` | Verlaat het huidige gesprek |
| `/delete` | | Verwijder het huidige gesprek (wijst openstaande berichtverzoeken af) |
| `/attach` | `/a` | Open de bestandsbrowser om een bestand bij te voegen |
| `/search <zoekterm>` | `/s` | Zoek berichten in het huidige gesprek (of in alle gesprekken) |
| `/sidebar` | `/sb` | Toon of verberg de zijbalk |
| `/bell [type]` | `/notify` | Meldingen in- of uitschakelen (`direct`, `group` of beide) |
| `/mute [duur]` | | Demp het huidige gesprek of hef het dempen op (bijv. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Blokkeer het huidige contact of de huidige groep |
| `/unblock` | | Deblokkeer het huidige contact of de huidige groep |
| `/disappearing <dur>` | `/dm` | Stel de timer voor verdwijnende berichten in (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Open het groepsbeheermenu |
| `/theme` | `/t` | Open de themakiezer |
| `/contacts` | `/c` | Blader door gesynchroniseerde contacten |
| `/settings` | | Open de instellingenoverlay |
| `/lock` | | Vergrendel de sessie |
| `/lock-reset` | | Wijzig de wachtwoordzin van de vergrendeling (huidige wachtwoordzin vereist) |
| `/help` | `/h` | Toon de hulpoverlay |
| `/quit` | `/q` | Sluit siggy af |

Typ `/` om de autocomplete-popup te openen. Gebruik `Tab` om aan te vullen en de pijltjestoetsen om te navigeren.

Een nieuw contact een bericht sturen: `/join +15551234567` (E.164-formaat).

**Wachtwoordzin van de vergrendeling vergeten?** Sluit siggy af (of beëindig het proces) en voer `siggy --reset-lock` uit. Dit verwijdert de opgeslagen hash van de wachtwoordzin en toont het pad dat is verwijderd. Bij de eerstvolgende `/lock` stel je een nieuwe wachtwoordzin in.

## Sneltoetsen

De app gebruikt modaal bewerken in vim-stijl met twee modi: **Insert** (standaard) en **Normal**.

### Globaal (beide modi)

| Toets | Actie |
|---|---|
| `Ctrl+C` | Afsluiten |
| `Tab` / `Shift+Tab` | Volgend / vorig gesprek |
| `PgUp` / `PgDn` | Door berichten scrollen (5 regels) |
| `Ctrl+Left` / `Ctrl+Right` | Zijbalk vergroten of verkleinen |

### Normal-modus

Druk op `Esc` om naar de Normal-modus te gaan.

| Toets | Actie |
|---|---|
| `j` / `k` | 1 regel omlaag / omhoog scrollen |
| `J` / `K` | Naar het vorige / volgende bericht springen |
| `Ctrl+D` / `Ctrl+U` | Een halve pagina omlaag / omhoog scrollen |
| `g` / `G` | Naar boven / beneden scrollen |
| `h` / `l` | Cursor naar links / rechts |
| `w` / `b` | Een woord vooruit / terug |
| `0` / `$` | Begin / einde van de regel |
| `x` | Teken onder de cursor verwijderen |
| `D` | Verwijderen vanaf de cursor tot het einde |
| `y` / `Y` | Berichttekst / volledige regel kopiëren |
| `r` | Reageren op het geselecteerde bericht |
| `q` | Het geselecteerde bericht beantwoorden / citeren |
| `e` | Eigen verzonden bericht bewerken |
| `d` | Bericht verwijderen (lokaal of op afstand) |
| `n` / `N` | Naar het volgende / vorige zoekresultaat springen |
| `i` | Naar de Insert-modus |
| `a` | Naar de Insert-modus (cursor 1 naar rechts) |
| `I` / `A` | Naar de Insert-modus aan het begin / einde van de regel |
| `o` | Naar de Insert-modus (buffer leegmaken) |
| `/` | Naar de Insert-modus met `/` al ingetypt |

### Insert-modus (standaard)

| Toets | Actie |
|---|---|
| `Esc` | Naar de Normal-modus |
| `Enter` | Bericht versturen / commando uitvoeren |
| `Shift+Enter` / `Alt+Enter` | Nieuwe regel invoegen (voor berichten van meerdere regels) |
| `Backspace` / `Delete` | Tekens verwijderen |
| `Up` / `Down` | Invoergeschiedenis terughalen |
| `Left` / `Right` | Cursor verplaatsen |
| `Home` / `End` | Naar het begin / einde van de regel |

## Architectuur

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

Gebouwd met [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) op de asynchrone [Tokio](https://tokio.rs/)-runtime.

## Licentie

[AGPL-3.0](../LICENSE)
