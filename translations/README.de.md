> This is the German translation of the siggy README.
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
  <b>Deutsch</b>
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
  <a href="../TRANSLATING.md">Eine Übersetzung beitragen</a>
</p>

Ein terminalbasierter Signal-Messenger-Client im IRC-Stil. Nutzt [signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC als Messaging-Backend.

![siggy Bildschirmfoto](../screenshot.png)

## Installation

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Vorkompilierte Binärdateien

Lade die neueste Version für deine Plattform von der [Releases-Seite](https://github.com/johnsideserf/siggy/releases) herunter.

**Linux / macOS** (Einzeiler):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Beide Skripte laden die neueste Binärdatei herunter und prüfen, ob signal-cli vorhanden ist.

### Über crates.io

Benötigt Rust 1.70+.

```sh
cargo install siggy
```

### Aus dem Quellcode bauen

Alternativ das Repository klonen und lokal kompilieren:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Die Binärdatei liegt unter target/release/siggy
```

## Voraussetzungen

- [signal-cli](https://github.com/AsamK/signal-cli) ist installiert und über den PATH erreichbar (oder per `signal_cli_path` konfiguriert)
- Ein Signal-Konto, das als Zweitgerät gekoppelt ist (der Einrichtungsassistent übernimmt das)

## Verwendung

```sh
siggy                        # Starten (verwendet die Konfigurationsdatei)
siggy -a +15551234567        # Konto angeben
siggy -c /path/to/config.toml  # Eigener Pfad zur Konfiguration
siggy --setup                # Einrichtungsassistenten erneut ausführen
siggy --demo                 # Mit Beispieldaten starten (kein signal-cli nötig)
siggy --incognito            # Keine lokale Nachrichtenspeicherung (nur im Arbeitsspeicher)
```

Beim ersten Start führt dich der Einrichtungsassistent durch das Auffinden von signal-cli, die Eingabe deiner Telefonnummer und die Kopplung deines Geräts per QR-Code.

## Konfiguration

Die Konfiguration wird geladen aus:
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

Alle Felder sind optional. `signal_cli_path` hat den Standardwert `"signal-cli"` (wird über den PATH gefunden), und `download_dir` zeigt standardmäßig auf `~/signal-downloads/`. Unter Windows gib den vollständigen Pfad zu `signal-cli.bat` an, falls die Datei nicht im PATH liegt.

### Inline-Bilder innerhalb von tmux

Außerhalb von tmux erkennt siggy Kitty / iTerm2 / WezTerm / Ghostty automatisch und rendert Anhänge als native Pixelbilder. Innerhalb von tmux sind zwei Schritte nötig, weil tmux das äußere Terminal vor siggy verbirgt:

1. tmux anweisen, unbekannte Escape-Sequenzen weiterzureichen. Erfordert tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Ältere tmux-Versionen verwenden `set -g allow-passthrough all`.

2. siggy mitteilen, welches Protokoll das äußere Terminal spricht (die automatische Erkennung sieht nur tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # oder iterm2 / sixel / halfblock
   ```

Ist `SIGGY_IMAGE_PROTOCOL` nicht gesetzt, greift die automatische Erkennung (außerhalb von tmux korrekt, innerhalb fällt sie auf halfblock zurück). Sixel wird ab tmux 3.4+ nativ durchgereicht und benötigt die Umgebungsvariable nicht.

## Funktionen

- **Messaging** -- Senden und Empfangen von Einzel- und Gruppennachrichten
- **Anhänge** -- Bildvorschauen werden inline als Halbblock-Grafik gerendert; andere Anhänge erscheinen als `[attachment: dateiname]`
- **Anklickbare Links** -- URLs und Dateipfade sind OSC 8-Hyperlinks (anklickbar in Terminals wie Windows Terminal, iTerm2 usw.)
- **Schreibanzeigen** -- Zeigt, wer gerade tippt, inklusive Auflösung des Kontaktnamens
- **Nachrichtensynchronisierung** -- Vom Telefon gesendete Nachrichten erscheinen in der TUI
- **Persistenz** -- Nachrichtenspeicherung in SQLite mit WAL-Modus; Unterhaltungen und Lesemarken überstehen Neustarts
- **Ungelesen-Zähler** -- Anzahl ungelesener Nachrichten in der Seitenleiste, mit Trennlinie "neue Nachrichten" im Chat
- **Benachrichtigungen** -- Terminalglocke bei neuen Nachrichten (getrennt einstellbar für Einzel- und Gruppenchats, Stummschaltung pro Unterhaltung) sowie Desktop-Benachrichtigungen auf Betriebssystemebene
- **Kontaktauflösung** -- Namen aus deinem Signal-Adressbuch; Gruppen werden beim Start automatisch geladen
- **Reaktionen** -- Mit `r` im Normal-Modus reagieren; Emoji-Auswahl mit Badge-Anzeige (`👍 2 ❤️ 1`)
- **Antworten / Zitieren** -- Drücke `q` auf einer fokussierten Nachricht, um mit zitiertem Kontext zu antworten
- **Nachrichten bearbeiten** -- Drücke `e`, um eigene gesendete Nachrichten zu bearbeiten
- **Nachrichten löschen** -- Drücke `d`, um lokal oder remote zu löschen (bei eigenen Nachrichten)
- **Unterhaltungen löschen** -- Mit `/delete` die aktuelle Unterhaltung lokal entfernen (lehnt offene Nachrichtenanfragen ab)
- **Nachrichtensuche** -- `/search <Suchbegriff>` mit `n`/`N`, um zwischen den Treffern zu springen
- **@-Erwähnungen** -- Tippe `@` in Gruppenchats, um Mitglieder mit Autovervollständigung zu erwähnen
- **Nachrichtenauswahl** -- Hervorhebung der fokussierten Nachricht beim Scrollen; `J`/`K`, um zwischen Nachrichten zu springen
- **Lesebestätigungen** -- Statussymbole an ausgehenden Nachrichten (Wird gesendet → Gesendet → Zugestellt → Gelesen → Angesehen)
- **Verschwindende Nachrichten** -- Berücksichtigt die Ablauf-Timer von Signal; pro Unterhaltung einstellbar mit `/disappearing`
- **Gruppenverwaltung** -- Gruppen erstellen, Mitglieder hinzufügen/entfernen, umbenennen, verlassen via `/group`
- **Nachrichtenanfragen** -- Nachrichten unbekannter Absender annehmen oder löschen
- **Blockieren / Freigeben** -- Kontakte oder Gruppen mit `/block` und `/unblock` blockieren bzw. wieder freigeben
- **Mausunterstützung** -- Unterhaltungen in der Seitenleiste anklicken, durch Nachrichten scrollen, Cursor per Klick positionieren
- **Farbschemata** -- Auswählbare Themes über `/theme` oder `/settings`
- **Einrichtungsassistent** -- Geführte Ersteinrichtung mit Gerätekopplung per QR-Code
- **Vim-Tastenbelegung** -- Modales Editieren (Normal/Insert) mit vollständiger Cursorsteuerung
- **Befehls-Autovervollständigung** -- Tab-Vervollständigung als Popup für Slash-Befehle
- **Einstellungs-Overlay** -- Benachrichtigungen, Seitenleiste und Inline-Bilder direkt in der App umschalten
- **Responsives Layout** -- Größenverstellbare Seitenleiste, die sich auf schmalen Terminals (<60 Spalten) automatisch ausblendet
- **Inkognito-Modus** -- `--incognito` speichert nur im Arbeitsspeicher; nach dem Beenden bleibt nichts zurück
- **Proxy-Unterstützung** -- Konfiguriere einen Signal-TLS-Proxy über das Konfigurationsfeld `proxy` für den Einsatz in eingeschränkten Netzwerken
- **Demo-Modus** -- Probiere die Oberfläche ohne signal-cli aus (`--demo`)

## Befehle

| Befehl | Alias | Beschreibung |
|---|---|---|
| `/join <name>` | `/j` | Zu einer Unterhaltung wechseln, per Kontaktname, Nummer oder Gruppe |
| `/part` | `/p` | Aktuelle Unterhaltung verlassen |
| `/delete` | | Aktuelle Unterhaltung löschen (lehnt offene Nachrichtenanfragen ab) |
| `/attach` | `/a` | Dateibrowser öffnen, um eine Datei anzuhängen |
| `/search <query>` | `/s` | Nachrichten in der aktuellen (oder allen) Unterhaltungen durchsuchen |
| `/sidebar` | `/sb` | Seitenleiste ein-/ausblenden |
| `/bell [type]` | `/notify` | Benachrichtigungen umschalten (`direct`, `group` oder beide) |
| `/mute [duration]` | | Aktuelle Unterhaltung stummschalten bzw. wieder aktivieren (z. B. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Aktuellen Kontakt oder aktuelle Gruppe blockieren |
| `/unblock` | | Blockierung des aktuellen Kontakts oder der aktuellen Gruppe aufheben |
| `/disappearing <dur>` | `/dm` | Timer für verschwindende Nachrichten setzen (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Menü zur Gruppenverwaltung öffnen |
| `/theme` | `/t` | Theme-Auswahl öffnen |
| `/contacts` | `/c` | Synchronisierte Kontakte durchblättern |
| `/settings` | | Einstellungs-Overlay öffnen |
| `/lock` | | Sitzung sperren |
| `/lock-reset` | | Sperr-Passphrase ändern (aktuelle Passphrase erforderlich) |
| `/help` | `/h` | Hilfe-Overlay anzeigen |
| `/quit` | `/q` | siggy beenden |

Tippe `/`, um das Autovervollständigungs-Popup zu öffnen. Mit `Tab` vervollständigen, mit den Pfeiltasten navigieren.

Um einem neuen Kontakt zu schreiben: `/join +15551234567` (E.164-Format).

**Sperr-Passphrase vergessen?** Beende siggy (oder beende den Prozess) und führe `siggy --reset-lock` aus. Der Befehl löscht den gespeicherten Passphrase-Hash und gibt den entfernten Pfad aus. Das nächste `/lock` legt eine neue Passphrase fest.

## Tastenkürzel

Die App nutzt modales Editieren im Vim-Stil mit zwei Modi: **Insert** (Standard) und **Normal**.

### Global (beide Modi)

| Taste | Aktion |
|---|---|
| `Ctrl+C` | Beenden |
| `Tab` / `Shift+Tab` | Nächste / vorherige Unterhaltung |
| `PgUp` / `PgDn` | Nachrichten scrollen (5 Zeilen) |
| `Ctrl+Left` / `Ctrl+Right` | Seitenleiste in der Größe anpassen |

### Normal-Modus

Drücke `Esc`, um in den Normal-Modus zu wechseln.

| Taste | Aktion |
|---|---|
| `j` / `k` | Eine Zeile nach unten / oben scrollen |
| `J` / `K` | Zur vorherigen / nächsten Nachricht springen |
| `Ctrl+D` / `Ctrl+U` | Eine halbe Seite nach unten / oben scrollen |
| `g` / `G` | Zum Anfang / Ende scrollen |
| `h` / `l` | Cursor nach links / rechts bewegen |
| `w` / `b` | Wortweise vor / zurück |
| `0` / `$` | Zeilenanfang / Zeilenende |
| `x` | Zeichen unter dem Cursor löschen |
| `D` | Vom Cursor bis zum Ende löschen |
| `y` / `Y` | Nachrichtentext / ganze Zeile kopieren |
| `r` | Auf die fokussierte Nachricht reagieren |
| `q` | Fokussierte Nachricht beantworten / zitieren |
| `e` | Eigene gesendete Nachricht bearbeiten |
| `d` | Nachricht löschen (lokal oder remote) |
| `n` / `N` | Zum nächsten / vorherigen Suchtreffer springen |
| `i` | In den Insert-Modus wechseln |
| `a` | In den Insert-Modus wechseln (Cursor eins nach rechts) |
| `I` / `A` | In den Insert-Modus am Zeilenanfang / Zeilenende wechseln |
| `o` | In den Insert-Modus wechseln (Eingabe leeren) |
| `/` | In den Insert-Modus wechseln, `/` ist bereits eingetippt |

### Insert-Modus (Standard)

| Taste | Aktion |
|---|---|
| `Esc` | In den Normal-Modus wechseln |
| `Enter` | Nachricht senden / Befehl ausführen |
| `Shift+Enter` / `Alt+Enter` | Zeilenumbruch einfügen (für mehrzeilige Nachrichten) |
| `Backspace` / `Delete` | Zeichen löschen |
| `Up` / `Down` | Eingabeverlauf abrufen |
| `Left` / `Right` | Cursor bewegen |
| `Home` / `End` | Zum Zeilenanfang / Zeilenende springen |

## Architektur

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

Gebaut mit [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) auf der asynchronen [Tokio](https://tokio.rs/)-Laufzeitumgebung.

## Lizenz

[AGPL-3.0](../LICENSE)
