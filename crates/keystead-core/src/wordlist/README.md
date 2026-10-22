# Passphrase word lists

Embedded with `include_str!` by `crates/keystead-core/src/generator.rs`
(`Wordlist::En` / `Wordlist::De`). One word per line, lowercase ASCII, no
duplicates. The entropy per word shown to users is `log2` of the real list
size (`Wordlist::bits_per_word`), so a changed list changes the numbers by
itself; the tests in `generator.rs` pin the sizes.

Both lists end up in every binary that links `keystead-core` (`Keystead.exe`
and the setup, `keystead-cli`, with Block 5 the Android APK). Their attribution
ships with them, not only with the source:

- in the app: Settings → *Über Keystead* → *Wortlisten für Passphrasen*
  (`about.wordlists`, `about.wordlistEn`, `about.wordlistDe` in
  `apps/desktop/src/i18n`) – the setup and the APK carry nothing else;
- in the portable ZIP: `docs/LIESMICH.txt`, section "4. Inhalte Dritter";
- in the repository: here and in the README credits (English and German).

Keep all of them in step when a list changes.

## `eff_large.txt` – English (`"en"`, the default)

- Source: EFF large word list for five dice,
  <https://www.eff.org/dice> (file `eff_large_wordlist.txt`), the dice
  numbers removed.
- 7776 words, log2(7776) ≈ 12.92 bit per word.
- © Electronic Frontier Foundation. Licence: Creative Commons Attribution
  4.0 International (CC BY 4.0,
  <https://creativecommons.org/licenses/by/4.0/>) – EFF's licence for its
  original material, <https://www.eff.org/copyright> (checked 2026-10-17).
  CC BY 4.0 §3(a) asks for the creator, a copyright notice, a link to the
  licence and to the material, and a note that it was changed, with every
  copy – binaries included; see the list above for where Keystead gives it.
  Keystead's change: the dice numbers were removed.

## `de_dys2p.txt` – German (`"de"`)

- Source: dys2p, "Mnemonische deutsche Wortlisten",
  <https://github.com/dys2p/wordlists-de>, file `de-7776-v1.txt`
  (last changed in upstream commit `5eba8aecb872a7dc390458eaa4a4ebdbcf88b70d`,
  2022-09-21; repository head when taken: `6ef31b9aefb8735a7b066592393d12843ec502cd`,
  2026-02-24). SHA-256 of the upstream file (LF line endings):
  `440fa02c65591328d6351435d3824c27b483a049f4eca0b13456d8c5090442e7`.
- Licence: the upstream `LICENSE` reads "This work is multi-licensed under
  Unlicense, CC0 and BSD-3. You can choose between one of them if you use
  this work." Keystead uses it under **CC0 1.0 Universal** (public domain
  dedication, <https://creativecommons.org/publicdomain/zero/1.0/>), which
  allows changing it and embedding it in software under any licence;
  attribution is not required but given here and in the README.
- Upstream properties: modelled on the EFF list; 7776 lowercase ASCII words
  (no ä, ö, ü, ß), mostly well-known nouns, verbs and adjectives in their base
  form, no proper names, meant to be unique from the fifth letter on
  (upstream says its rules are followed mostly, not perfectly). Measured on
  `de_dys2p.txt`: 5 to 19 letters per word; the first five letters tell all
  words apart except three pairs (gelacht/gelackmeiert,
  metapher/metaphorisch, scheren/scherzen) – irrelevant for the generator,
  which always writes whole words.
- **Keystead's change:** 52 words were removed so that passphrases stay
  family-friendly (vulgar, sexual, violent or Nazi-era terms, slurs and
  stigmatising words). What remains: **7724 words**, log2(7724) ≈ 12.92 bit
  per word (0.0097 bit less than 7776). SHA-256 of `de_dys2p.txt` (LF line
  endings): `83e98e43bc87ff54c661e594cccdca8032cb4b0759b1bb89c539e57d59b2f5cf`.

  Removed: abartig, abtreiben, amputation, amtsmissbrauch, arsch, asozial,
  attentat, beischlaf, besoffen, bestialisch, darmentleerung, deportation,
  deppen, ehebruch, endsieg, entkleidet, erdrosselt, ermorden, erstach,
  ersticht, erstochen, exkremente, fallbeil, galgen, gelyncht, gemobbt,
  geoutet, gepeinigt, gesindel, gesoffen, gutmenschen, halbnackt, hetzjagd,
  hetzkampagne, intim, invalide, kotzen, lasziv, leitkultur, nackt, notdurft,
  peinigen, pulsadern, qualvoll, sadistisch, saufen, sippenhaft, soziopath,
  umerziehung, unzucht, vollrausch, wollust.

  To rebuild: save the list above one word per line as `removed.txt`, then
  `grep -vxFf removed.txt de-7776-v1.txt > de_dys2p.txt`.
