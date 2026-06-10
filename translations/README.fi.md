> This is the Finnish translation of the siggy README.
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
  <a href="README.pt-BR.md">Português</a>
  &nbsp;|&nbsp;
  <b>Suomi</b>
  &nbsp;|&nbsp;
  <a href="README.sv.md">Svenska</a>
  &nbsp;|&nbsp;
  <a href="README.ru.md">Русский</a>
  &nbsp;|&nbsp;
  <a href="README.uk.md">Українська</a>
  &nbsp;|&nbsp;
  <a href="README.zh-CN.md">简体中文</a>
  &nbsp;|&nbsp;
  <a href="../TRANSLATING.md">Osallistu kääntämiseen</a>
</p>

Päätteessä toimiva Signal-viestintäsovellus IRC-henkisellä ulkoasulla. Käyttää [signal-cli](https://github.com/AsamK/signal-cli)-ohjelmaa JSON-RPC:n kautta viestinnän taustapalveluna.

![Kuvakaappaus siggystä](../screenshot.png)

## Asennus

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Valmiit binaarit

Lataa uusin julkaisu omalle alustallesi [Releases](https://github.com/johnsideserf/siggy/releases)-sivulta.

**Linux / macOS** (yksi komento):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Molemmat skriptit lataavat uusimman julkaisubinaarin ja tarkistavat signal-cli:n saatavuuden.

### crates.io:sta

Vaatii Rustin version 1.70 tai uudemman.

```sh
cargo install siggy
```

### Kääntäminen lähdekoodista

Voit myös kloonata repositorion ja kääntää sen itse:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Binaari löytyy polusta target/release/siggy
```

## Esivaatimukset

- [signal-cli](https://github.com/AsamK/signal-cli) asennettuna ja käytettävissä PATH-polulla (tai määritettynä `signal_cli_path`-asetuksella)
- Signal-tili liitettynä toissijaiseksi laitteeksi (ohjattu käyttöönotto hoitaa tämän)

## Käyttö

```sh
siggy                        # Käynnistä (käyttää asetustiedostoa)
siggy -a +15551234567        # Määritä tili
siggy -c /path/to/config.toml  # Mukautettu asetustiedoston polku
siggy --setup                # Aja ohjattu käyttöönotto uudelleen
siggy --demo                 # Käynnistä esimerkkidatalla (ei vaadi signal-cli:tä)
siggy --incognito            # Ei paikallista viestitallennusta (vain muistissa)
```

Ensimmäisellä käynnistyskerralla ohjattu käyttöönotto opastaa sinut signal-cli:n paikantamisen, puhelinnumeron syöttämisen ja laitteen QR-koodilla liittämisen läpi.

## Asetukset

Asetukset ladataan tiedostosta:
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

Kaikki kentät ovat valinnaisia. `signal_cli_path` on oletuksena `"signal-cli"` (haetaan PATH-polulta), ja `download_dir` on oletuksena `~/signal-downloads/`. Käytä Windowsissa täyttä polkua `signal-cli.bat`-tiedostoon, jos se ei ole PATH-polullasi.

### Upotetut kuvat tmuxin sisällä

tmuxin ulkopuolella siggy tunnistaa automaattisesti Kitty-, iTerm2-, WezTerm- ja Ghostty-päätteet ja näyttää liitteet natiiveina pikselikuvina. tmuxin sisällä tarvitaan kaksi asetusta, koska tmux piilottaa ulomman päätteen siggyltä:

1. Käske tmuxia välittämään tuntemattomat ohjaussekvenssit eteenpäin. Vaatii tmux-version 3.3+:

   ```
   set -g allow-passthrough on
   ```

   Vanhemmissa tmux-versioissa käytetään asetusta `set -g allow-passthrough all`.

2. Kerro siggylle, mitä protokollaa ulompi pääte puhuu (automaattinen tunnistus näkee vain tmuxin):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # tai iterm2 / sixel / halfblock
   ```

Jos `SIGGY_IMAGE_PROTOCOL`-muuttujaa ei ole asetettu, käytössä on tavallinen automaattinen tunnistus (toimii oikein tmuxin ulkopuolella, tmuxin sisällä siirrytään halfblock-tilaan). Sixel kulkee tmux 3.4+:n läpi natiivisti eikä tarvitse ympäristömuuttujaa.

## Ominaisuudet

- **Viestintä** -- Lähetä ja vastaanota kahdenkeskisiä viestejä ja ryhmäviestejä
- **Liitteet** -- Kuvien esikatselut näytetään keskustelussa halfblock-grafiikkana; muut kuin kuvaliitteet näkyvät muodossa `[attachment: tiedostonimi]`
- **Klikattavat linkit** -- URL-osoitteet ja tiedostopolut ovat OSC 8 -hyperlinkkejä (klikattavissa esimerkiksi Windows Terminalissa ja iTerm2:ssa)
- **Kirjoitusilmaisimet** -- Näyttää yhteystiedon nimen perusteella, kuka kirjoittaa parhaillaan
- **Viestien synkronointi** -- Puhelimellasi lähetetyt viestit näkyvät TUI:ssa
- **Pysyvä tallennus** -- Viestit tallennetaan SQLite-tietokantaan WAL-tilassa; keskustelut ja lukumerkinnät säilyvät uudelleenkäynnistysten yli
- **Lukemattomien seuranta** -- Lukemattomien viestien määrä sivupalkissa ja "uudet viestit" -erotin keskustelussa
- **Ilmoitukset** -- Päätteen äänimerkki uusista viesteistä (säädettävissä erikseen yksityis- ja ryhmäviesteille, keskustelukohtainen mykistys) sekä käyttöjärjestelmän työpöytäilmoitukset
- **Yhteystietojen tunnistus** -- Nimet haetaan Signal-osoitekirjastasi; ryhmät täytetään automaattisesti käynnistyksen yhteydessä
- **Viestireaktiot** -- Reagoi `r`-näppäimellä Normal-tilassa; emojivalitsin ja reaktiomerkit (`👍 2 ❤️ 1`)
- **Vastaa / lainaa** -- Paina `q` valitun viestin kohdalla vastataksesi lainatulla kontekstilla
- **Viestien muokkaus** -- Paina `e` muokataksesi omia lähetettyjä viestejäsi
- **Viestien poisto** -- Paina `d` poistaaksesi viestin paikallisesti tai etänä (omat viestisi)
- **Keskustelujen poisto** -- Poista nykyinen keskustelu paikallisesti komennolla `/delete` (hylkää avoimet viestipyynnöt)
- **Viestihaku** -- `/search <hakusana>`; siirry osumien välillä näppäimillä `n`/`N`
- **@maininnat** -- Kirjoita `@` ryhmäkeskustelussa mainitaksesi jäseniä automaattitäydennyksen avulla
- **Viestin valinta** -- Valittu viesti korostetaan vieritettäessä; `J`/`K` hyppäävät viestistä toiseen
- **Lukukuittaukset** -- Tilasymbolit lähtevissä viesteissä (Lähetetään → Lähetetty → Toimitettu → Luettu → Katsottu)
- **Katoavat viestit** -- Noudattaa Signalin katoavien viestien ajastimia; asetetaan keskustelukohtaisesti komennolla `/disappearing`
- **Ryhmien hallinta** -- Luo ryhmiä, lisää ja poista jäseniä, nimeä uudelleen ja poistu ryhmistä komennolla `/group`
- **Viestipyynnöt** -- Hyväksy tai poista tuntemattomilta lähettäjiltä tulevat viestit
- **Esto / eston poisto** -- Estä yhteystietoja tai ryhmiä komennoilla `/block` ja `/unblock`
- **Hiirituki** -- Klikkaa keskusteluja sivupalkissa, vieritä viestejä, klikkaa siirtääksesi kohdistinta
- **Väriteemat** -- Valittavat teemat komennolla `/theme` tai `/settings`
- **Ohjattu käyttöönotto** -- Ensimmäisen käynnistyksen opastus ja laitteen liittäminen QR-koodilla
- **Vim-näppäinkomennot** -- Modaalinen muokkaus (Normal/Insert) täysillä kohdistinliikkeillä
- **Komentojen automaattitäydennys** -- Tab-täydennys ponnahdusikkunassa slash-komennoille
- **Asetuspaneeli** -- Ilmoitukset, sivupalkki ja upotetut kuvat kytkettävissä päälle ja pois sovelluksen sisältä
- **Mukautuva asettelu** -- Sivupalkin kokoa voi muuttaa, ja se piiloutuu automaattisesti kapeissa päätteissä (<60 saraketta)
- **Incognito-tila** -- `--incognito` käyttää muistinvaraista tallennusta; mitään ei säily sulkemisen jälkeen
- **Välityspalvelintuki** -- Määritä Signalin TLS-välityspalvelin `proxy`-asetuksella rajoitettuja verkkoja varten
- **Demotila** -- Kokeile käyttöliittymää ilman signal-cli:tä (`--demo`)

## Komennot

| Komento | Alias | Kuvaus |
|---|---|---|
| `/join <name>` | `/j` | Siirry keskusteluun yhteystiedon nimen, numeron tai ryhmän perusteella |
| `/part` | `/p` | Poistu nykyisestä keskustelusta |
| `/delete` | | Poista nykyinen keskustelu (hylkää avoimet viestipyynnöt) |
| `/attach` | `/a` | Avaa tiedostoselain liitteen lisäämistä varten |
| `/search <query>` | `/s` | Hae viestejä nykyisestä keskustelusta (tai kaikista keskusteluista) |
| `/sidebar` | `/sb` | Näytä tai piilota sivupalkki |
| `/bell [type]` | `/notify` | Kytke ilmoitukset päälle tai pois (`direct`, `group` tai molemmat) |
| `/mute [duration]` | | Mykistä nykyinen keskustelu tai poista mykistys (esim. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Estä nykyinen yhteystieto tai ryhmä |
| `/unblock` | | Poista nykyisen yhteystiedon tai ryhmän esto |
| `/disappearing <dur>` | `/dm` | Aseta katoavien viestien ajastin (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Avaa ryhmänhallintavalikko |
| `/theme` | `/t` | Avaa teemavalitsin |
| `/contacts` | `/c` | Selaa synkronoituja yhteystietoja |
| `/settings` | | Avaa asetuspaneeli |
| `/lock` | | Lukitse istunto |
| `/lock-reset` | | Vaihda lukituksen tunnuslause (vaatii nykyisen tunnuslauseen) |
| `/help` | `/h` | Näytä ohjepaneeli |
| `/quit` | `/q` | Sulje siggy |

Kirjoita `/` avataksesi automaattitäydennyksen ponnahdusikkunan. Täydennä `Tab`-näppäimellä ja liiku nuolinäppäimillä.

Viestin lähettäminen uudelle yhteystiedolle: `/join +15551234567` (E.164-muoto).

**Unohditko lukituksen tunnuslauseen?** Sulje siggy (tai lopeta prosessi) ja aja `siggy --reset-lock`. Komento poistaa tallennetun tunnuslauseen tiivisteen ja tulostaa poistamansa tiedoston polun. Seuraava `/lock` asettaa uuden tunnuslauseen.

## Pikanäppäimet

Sovellus käyttää vim-tyylistä modaalista muokkausta kahdella tilalla: **Insert** (oletus) ja **Normal**.

### Yleiset (molemmissa tiloissa)

| Näppäin | Toiminto |
|---|---|
| `Ctrl+C` | Lopeta |
| `Tab` / `Shift+Tab` | Seuraava / edellinen keskustelu |
| `PgUp` / `PgDn` | Vieritä viestejä (5 riviä) |
| `Ctrl+Left` / `Ctrl+Right` | Muuta sivupalkin kokoa |

### Normal-tila

Siirry Normal-tilaan painamalla `Esc`.

| Näppäin | Toiminto |
|---|---|
| `j` / `k` | Vieritä alas / ylös yksi rivi |
| `J` / `K` | Hyppää edelliseen / seuraavaan viestiin |
| `Ctrl+D` / `Ctrl+U` | Vieritä alas / ylös puoli sivua |
| `g` / `G` | Vieritä alkuun / loppuun |
| `h` / `l` | Siirrä kohdistinta vasemmalle / oikealle |
| `w` / `b` | Sana eteenpäin / taaksepäin |
| `0` / `$` | Rivin alkuun / loppuun |
| `x` | Poista merkki kohdistimen kohdalta |
| `D` | Poista kohdistimesta rivin loppuun |
| `y` / `Y` | Kopioi viestin sisältö / koko rivi |
| `r` | Reagoi valittuun viestiin |
| `q` | Vastaa / lainaa valittua viestiä |
| `e` | Muokkaa omaa lähetettyä viestiä |
| `d` | Poista viesti (paikallisesti tai etänä) |
| `n` / `N` | Hyppää seuraavaan / edelliseen hakuosumaan |
| `i` | Siirry Insert-tilaan |
| `a` | Siirry Insert-tilaan (kohdistin yhden askeleen oikealle) |
| `I` / `A` | Siirry Insert-tilaan rivin alkuun / loppuun |
| `o` | Siirry Insert-tilaan (tyhjennä syöttökenttä) |
| `/` | Siirry Insert-tilaan `/` valmiiksi kirjoitettuna |

### Insert-tila (oletus)

| Näppäin | Toiminto |
|---|---|
| `Esc` | Siirry Normal-tilaan |
| `Enter` | Lähetä viesti / suorita komento |
| `Shift+Enter` / `Alt+Enter` | Lisää rivinvaihto (monirivisiä viestejä varten) |
| `Backspace` / `Delete` | Poista merkkejä |
| `Up` / `Down` | Selaa syöttöhistoriaa |
| `Left` / `Right` | Siirrä kohdistinta |
| `Home` / `End` | Hyppää rivin alkuun / loppuun |

## Arkkitehtuuri

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

Toteutettu [Ratatui](https://ratatui.rs/)- ja [Crossterm](https://github.com/crossterm-rs/crossterm)-kirjastoilla [Tokio](https://tokio.rs/)-pohjaisen asynkronisen ajoympäristön päällä.

## Lisenssi

[GPL-3.0](../LICENSE)
