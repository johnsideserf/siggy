> This is the Italian translation of the siggy README.
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
  <b>Italiano</b>
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
  <a href="../TRANSLATING.md">Contribuisci con una traduzione</a>
</p>

Un client Signal per il terminale, in stile IRC. Si appoggia a [signal-cli](https://github.com/AsamK/signal-cli) tramite JSON-RPC come backend di messaggistica.

![Schermata di siggy](../screenshot.png)

## Installazione

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Binari precompilati

Scarica l'ultima versione per la tua piattaforma dalla pagina [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (una sola riga):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Entrambi gli script scaricano il binario dell'ultima release e verificano la presenza di signal-cli.

### Da crates.io

Richiede Rust 1.70+.

```sh
cargo install siggy
```

### Compilazione dal sorgente

In alternativa, clona il repository e compila in locale:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Il binario si trova in target/release/siggy
```

## Prerequisiti

- [signal-cli](https://github.com/AsamK/signal-cli) installato e raggiungibile nel PATH (oppure configurato tramite `signal_cli_path`)
- Un account Signal collegato come dispositivo secondario (la procedura guidata di configurazione se ne occupa)

## Utilizzo

```sh
siggy                        # Avvio (usa il file di configurazione)
siggy -a +15551234567        # Specifica l'account
siggy -c /path/to/config.toml  # Percorso di configurazione personalizzato
siggy --setup                # Riesegui la configurazione guidata iniziale
siggy --demo                 # Avvio con dati fittizi (signal-cli non necessario)
siggy --incognito            # Nessun salvataggio locale dei messaggi (solo in memoria)
```

Al primo avvio, la procedura guidata ti accompagna nell'individuare signal-cli, inserire il tuo numero di telefono e collegare il dispositivo tramite codice QR.

## Configurazione

La configurazione viene caricata da:
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
```

Tutti i campi sono facoltativi. Il valore predefinito di `signal_cli_path` è `"signal-cli"` (cercato nel PATH), mentre quello di `download_dir` è `~/signal-downloads/`. Su Windows, indica il percorso completo di `signal-cli.bat` se non si trova nel PATH.

### Immagini inline dentro tmux

Fuori da tmux, siggy rileva automaticamente Kitty / iTerm2 / WezTerm / Ghostty e visualizza gli allegati come immagini native in pixel. Dentro tmux servono due passaggi, perché tmux nasconde a siggy il terminale esterno:

1. Indica a tmux di inoltrare le sequenze di escape sconosciute. Richiede tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Le versioni meno recenti di tmux usano `set -g allow-passthrough all`.

2. Indica a siggy quale protocollo parla il terminale esterno (il rilevamento automatico vede solo tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # oppure iterm2 / sixel / halfblock
   ```

Se `SIGGY_IMAGE_PROTOCOL` non è impostata, entra in gioco il consueto rilevamento automatico (corretto fuori da tmux, ripiega su halfblock al suo interno). Sixel attraversa tmux 3.4+ in modo nativo e non richiede la variabile d'ambiente.

## Funzionalità

- **Messaggistica** -- Invio e ricezione di messaggi individuali e di gruppo
- **Allegati** -- Anteprime delle immagini visualizzate inline come grafica a semiblocchi; gli allegati non grafici compaiono come `[attachment: nomefile]`
- **Link cliccabili** -- URL e percorsi di file sono collegamenti ipertestuali OSC 8 (cliccabili in terminali come Windows Terminal, iTerm2, ecc.)
- **Indicatori di digitazione** -- Mostra chi sta scrivendo, con risoluzione del nome del contatto
- **Sincronizzazione dei messaggi** -- I messaggi inviati dal telefono compaiono nella TUI
- **Persistenza** -- Archiviazione dei messaggi in SQLite con modalità WAL; conversazioni e segnalibri di lettura sopravvivono ai riavvii
- **Tracciamento dei non letti** -- Conteggio dei messaggi non letti nella barra laterale, con separatore "nuovi messaggi" nella chat
- **Notifiche** -- Campanello del terminale ai nuovi messaggi (configurabile per chat individuali/di gruppo, silenziabile per singola conversazione) e notifiche desktop a livello di sistema operativo
- **Risoluzione dei contatti** -- Nomi presi dalla tua rubrica Signal; gruppi popolati automaticamente all'avvio
- **Reazioni ai messaggi** -- Reagisci con `r` in modalità Normale; selettore di emoji con visualizzazione a badge (`👍 2 ❤️ 1`)
- **Rispondi / cita** -- Premi `q` su un messaggio selezionato per rispondere citandone il contesto
- **Modifica dei messaggi** -- Premi `e` per modificare i messaggi che hai inviato
- **Eliminazione dei messaggi** -- Premi `d` per eliminare in locale o in remoto (per i tuoi messaggi)
- **Eliminazione delle conversazioni** -- Usa `/delete` per rimuovere localmente la conversazione corrente (rifiuta le richieste di messaggio in sospeso)
- **Ricerca nei messaggi** -- `/search <query>` con `n`/`N` per saltare da un risultato all'altro
- **@menzioni** -- Digita `@` nelle chat di gruppo per menzionare i membri con autocompletamento
- **Selezione dei messaggi** -- Evidenziazione del messaggio selezionato durante lo scorrimento; `J`/`K` per saltare da un messaggio all'altro
- **Conferme di lettura** -- Simboli di stato sui messaggi in uscita (In invio → Inviato → Consegnato → Letto → Visualizzato)
- **Messaggi a tempo** -- Rispetta i timer dei messaggi a tempo di Signal; impostabili per conversazione con `/disappearing`
- **Gestione dei gruppi** -- Crea gruppi, aggiungi/rimuovi membri, rinomina, abbandona tramite `/group`
- **Richieste di messaggio** -- Accetta o elimina i messaggi provenienti da mittenti sconosciuti
- **Blocca / sblocca** -- Blocca contatti o gruppi con `/block` e `/unblock`
- **Supporto del mouse** -- Clicca sulle conversazioni nella barra laterale, scorri i messaggi, clicca per posizionare il cursore
- **Temi di colore** -- Temi selezionabili tramite `/theme` o `/settings`
- **Configurazione guidata** -- Onboarding al primo avvio con collegamento del dispositivo via codice QR
- **Scorciatoie Vim** -- Editing modale (Normal/Insert) con movimento completo del cursore
- **Autocompletamento dei comandi** -- Popup di completamento con Tab per i comandi slash
- **Overlay delle impostazioni** -- Attiva/disattiva notifiche, barra laterale e immagini inline direttamente dall'app
- **Layout adattivo** -- Barra laterale ridimensionabile che si nasconde automaticamente sui terminali stretti (<60 colonne)
- **Modalità incognito** -- `--incognito` usa l'archiviazione in memoria; nulla viene conservato dopo l'uscita
- **Modalità demo** -- Prova l'interfaccia senza signal-cli (`--demo`)

## Comandi

| Comando | Alias | Descrizione |
|---|---|---|
| `/join <nome>` | `/j` | Passa a una conversazione per nome del contatto, numero o gruppo |
| `/part` | `/p` | Abbandona la conversazione corrente |
| `/delete` | | Elimina la conversazione corrente (rifiuta le richieste di messaggio in sospeso) |
| `/attach` | `/a` | Apri il browser dei file per allegare un file |
| `/search <query>` | `/s` | Cerca messaggi nella conversazione corrente (o in tutte) |
| `/sidebar` | `/sb` | Mostra/nascondi la barra laterale |
| `/bell [tipo]` | `/notify` | Attiva/disattiva le notifiche (`direct`, `group` o entrambe) |
| `/mute [durata]` | | Silenzia/riattiva la conversazione corrente (ad es. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Blocca il contatto o il gruppo corrente |
| `/unblock` | | Sblocca il contatto o il gruppo corrente |
| `/disappearing <dur>` | `/dm` | Imposta il timer dei messaggi a tempo (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Apri il menu di gestione del gruppo |
| `/theme` | `/t` | Apri il selettore dei temi |
| `/contacts` | `/c` | Sfoglia i contatti sincronizzati |
| `/settings` | | Apri l'overlay delle impostazioni |
| `/lock` | | Blocca la sessione |
| `/lock-reset` | | Cambia la passphrase di blocco (richiede quella attuale) |
| `/help` | `/h` | Mostra l'overlay di aiuto |
| `/quit` | `/q` | Esci da siggy |

Digita `/` per aprire il popup di autocompletamento. Usa `Tab` per completare e i tasti freccia per navigare.

Per scrivere a un nuovo contatto: `/join +15551234567` (formato E.164).

**Hai dimenticato la passphrase di blocco?** Esci da siggy (o termina il processo) ed esegui `siggy --reset-lock`. Il comando elimina l'hash della passphrase memorizzata e stampa il percorso rimosso. Il successivo `/lock` imposterà una nuova passphrase.

## Scorciatoie da tastiera

L'app usa un editing modale in stile vim con due modalità: **Inserimento** (predefinita) e **Normale**.

### Globali (entrambe le modalità)

| Tasto | Azione |
|---|---|
| `Ctrl+C` | Esci |
| `Tab` / `Shift+Tab` | Conversazione successiva / precedente |
| `PgUp` / `PgDn` | Scorri i messaggi (5 righe) |
| `Ctrl+Left` / `Ctrl+Right` | Ridimensiona la barra laterale |

### Modalità Normale

Premi `Esc` per entrare in modalità Normale.

| Tasto | Azione |
|---|---|
| `j` / `k` | Scorri giù / su di 1 riga |
| `J` / `K` | Salta al messaggio precedente / successivo |
| `Ctrl+D` / `Ctrl+U` | Scorri giù / su di mezza pagina |
| `g` / `G` | Scorri all'inizio / alla fine |
| `h` / `l` | Sposta il cursore a sinistra / a destra |
| `w` / `b` | Avanza / arretra di una parola |
| `0` / `$` | Inizio / fine riga |
| `x` | Elimina il carattere sotto il cursore |
| `D` | Elimina dal cursore fino alla fine |
| `y` / `Y` | Copia il corpo del messaggio / la riga intera |
| `r` | Reagisci al messaggio selezionato |
| `q` | Rispondi / cita il messaggio selezionato |
| `e` | Modifica un proprio messaggio inviato |
| `d` | Elimina il messaggio (in locale o in remoto) |
| `n` / `N` | Salta al risultato di ricerca successivo / precedente |
| `i` | Entra in modalità Inserimento |
| `a` | Entra in modalità Inserimento (cursore a destra di 1) |
| `I` / `A` | Entra in modalità Inserimento a inizio / fine riga |
| `o` | Entra in modalità Inserimento (svuota il buffer) |
| `/` | Entra in modalità Inserimento con `/` già digitato |

### Modalità Inserimento (predefinita)

| Tasto | Azione |
|---|---|
| `Esc` | Passa alla modalità Normale |
| `Enter` | Invia il messaggio / esegui il comando |
| `Shift+Enter` / `Alt+Enter` | Inserisci un a capo (per messaggi su più righe) |
| `Backspace` / `Delete` | Elimina caratteri |
| `Up` / `Down` | Richiama la cronologia di input |
| `Left` / `Right` | Sposta il cursore |
| `Home` / `End` | Vai all'inizio / alla fine della riga |

## Architettura

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

Sviluppato con [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) sul runtime asincrono [Tokio](https://tokio.rs/).

## Licenza

[AGPL-3.0](../LICENSE)
