# Translating siggy

Thanks for helping translate siggy. Non-English translations lower the barrier for a big chunk of Signal's international user base, and this work is genuinely appreciated.

## Scope (and what is deferred)

Right now, siggy accepts **README translations only**. The full mdBook documentation site at [siggy.chat](https://siggy.chat/) stays English-only for now. Once a few README translations land and we have a sense of which languages have active maintainers, I will open a follow-up issue to restructure the mdBook for multi-language builds. Start with the README.

The app UI itself is not currently translatable. That is a separate, larger effort (see open issues for i18n of the TUI).

## Priority languages

Based on Signal's user distribution, the highest-impact translations are:

**Tier 1 (largest Signal user bases):**
1. German (`de`) - largest single market
2. French (`fr`) - France, Belgium, Canada
3. Ukrainian (`uk`)
4. Dutch (`nl`) - Signal is the #1 app in the Netherlands
5. Spanish (`es`) - broad global reach

**Tier 2 (strong regional demand):**
6. Italian (`it`)
7. Arabic (`ar`) - Egypt, Saudi Arabia, UAE (right-to-left, please test rendering)
8. Persian / Farsi (`fa`) - Iran (right-to-left)
9. Finnish (`fi`)
10. Swedish (`sv`), Danish (`da`)

Other languages are welcome. If you want to contribute one not on this list, just open a PR.

## Naming convention

Translated READMEs live in the repo root with a two-letter [ISO 639-1](https://en.wikipedia.org/wiki/List_of_ISO_639-1_codes) language code:

- `README.md` (English, authoritative)
- `README.de.md` (German)
- `README.fr.md` (French)
- `README.zh-CN.md` (Simplified Chinese - use `zh-CN` / `zh-TW` when a distinction matters)

Always link back to the English authoritative version from the top of your translation.

## Required header

Every translated README must start with this block so readers know whether the translation is current:

```markdown
> This is the <LANGUAGE> translation of the siggy README.
> Last updated against English commit: <SHA>
> The [English version](README.md) is authoritative. If this translation has drifted, trust the English.
```

Fill in `<LANGUAGE>` with the language name in English (so cross-language readers understand) and `<SHA>` with the short commit hash of the English README your translation was based on. You can find it with:

```sh
git log -1 --format=%h README.md
```

## Language switcher

When you submit a new translation, update the switcher at the top of `README.md` to include your language. The pattern:

```html
<p align="center">
  <a href="README.md">English</a>
  &nbsp;|&nbsp;
  <a href="README.de.md">Deutsch</a>
  &nbsp;|&nbsp;
  <a href="TRANSLATING.md">Contribute a translation</a>
</p>
```

Use the endonym (the language's own name for itself) - `Deutsch`, `Français`, `Español`, `Українська`, `العربية`, etc. - not the English name.

Mirror the same switcher in each translated README, with the current language in bold (no link).

## Translation quality

- **Native speakers only.** Please do not submit machine-translated READMEs unless you are a native speaker who has reviewed and corrected the output end-to-end. A rough machine translation is worse than none at all - it gives readers false confidence and makes the project look careless.
- Technical terms (signal-cli, TUI, JSON-RPC, crates.io, Homebrew) stay in English.
- Code blocks and CLI commands stay in English.
- Image alt text should be translated.

## Maintaining a translation

Translations drift. When the English README changes, yours will eventually go stale. That is expected and acceptable as long as the staleness is visible:

- I will **not** gate English PRs on translation updates. English changes ship first.
- Translated READMEs carry the `Last updated against English commit: <SHA>` header so readers can see how stale the translation is.
- If you want to own a language long-term, mention that in your PR. You will be tagged on future translation-sync requests. Ownership is informal and you can hand it off any time.
- If a translation goes more than six months without a sync and no one steps up, I may remove it to avoid shipping stale content. This is not a rule, just a heuristic.

## Submitting

1. Fork and branch off `master`. Branch name: `docs/translate-<lang>` (e.g. `docs/translate-de`).
2. Copy `README.md` to `README.<lang>.md` and translate it.
3. Add the required header and update the language switcher in `README.md`.
4. Open a PR titled `docs: add <language> README translation` and link issue #353.
5. I will review for formatting, not linguistic accuracy - I trust the translator on the prose. I may ask another speaker of the language to sanity-check if one happens to be available.

Questions? Open a discussion or comment on issue #353.
