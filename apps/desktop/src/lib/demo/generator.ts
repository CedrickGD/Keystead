// Password / passphrase generator and the built-in presets for the mock
// backend (the real ones live in keystead-core's generator.rs). The option
// checks come from ../generatorRules.ts (the same codes in the same order as
// the core); randomness from crypto.getRandomValues with rejection sampling,
// so every pick is uniform.

import { DEFAULT_GENERATOR_OPTIONS, type GeneratorOptions, type GeneratorPreset, type Wordlist } from "../types";
import { normalizedOptions, passphraseIssue, passwordPlan, PRESET_NAME_MAX_CHARS } from "../generatorRules";

// Short lists of easy words. The real app embeds the EFF large list (7776
// words) and a German list derived from dys2p's de-7776 (7724 words); the
// page computes the entropy from those real sizes, not from these.
const WORDS_EN = (
  "acorn actor adapt agent alarm album alert alien alpine amber anchor angle ankle apple april arena armor arrow " +
  "atlas attic audio august autumn avocado bacon badge bagel baker balmy bamboo banjo barn basil basket beacon " +
  "beaver bench berry bicycle bison blanket blossom blue bonsai border bottle brave breeze brick bridge broccoli " +
  "bubble bucket buffalo butter cabin cactus camera canal candle canoe canyon carbon cargo carpet castle cedar " +
  "cello chalk cherry chess chimney cider cinema circus citrus clover cobalt coconut comet compass copper coral " +
  "cosmos cotton crayon cricket crystal cupcake cycle dancer dawn delta denim desert diamond dinner dolphin domino " +
  "dragon drum eagle echo eclipse elbow ember emerald engine falcon fern fiddle figure finch fjord flame flute " +
  "forest fossil fox galaxy garden garlic gecko ginger glacier globe gopher granite grape gravel guitar hammock " +
  "harbor harvest hazel helmet heron honey horizon husky igloo island ivory jacket jaguar jasmine jelly jigsaw " +
  "jungle kayak kettle kiwi koala ladder lagoon lantern lava lemon lily linen lobster lotus lunar magnet mango " +
  "maple marble meadow melon meteor mint mirror mocha monsoon mosaic muffin nectar needle nickel noodle nutmeg " +
  "oasis ocean olive onion orbit orchid otter oyster paddle panda papaya parrot pebble pepper piano pickle pilot " +
  "pine planet plum polar pony poppy prism pumpkin puzzle quartz quill rabbit radar radish raven reef ribbon " +
  "river robin rocket saddle saffron salmon sandal saturn scarf shadow sierra silver sketch sleet snorkel sonnet " +
  "spark spruce squash staple steam stone summit sunset swan tango teapot temple thistle thunder tiger timber " +
  "toast topaz tornado trumpet tulip tundra turtle umbrella unicorn valley velvet violet volcano waffle walnut " +
  "willow window winter wizard yogurt zebra zephyr zigzag"
).split(" ");

const WORDS_DE = (
  "abend adler ahorn ampel anker apfel april atlas auge bach backen bagger banane bank bauer baum beere berg biene " +
  "birne blatt blume boot brille brot brunnen buch burg butter dach dampf decke delfin donner dorf drache dusche " +
  "eiche eimer eis elster engel ente erbse esel eule fahne falke farbe feder fenster ferien fichte fisch flasche " +
  "flocke floh flur fluss fohlen forelle frosch fuchs gabel garten geige gipfel glocke gold gras hafen hagel hammer " +
  "hase haus hecke heft himmel hirsch honig horn hummel hut igel insel jacke jaguar juli kabel kaffee kamel kanu " +
  "karte kater kerze kiefer kirsche kiste klee knopf koffer komet korb kran krone kuchen kugel lampe laterne laub " +
  "leiter lerche linde loewe loeffel luchs mais mantel marder meer melone milch minze moewe mond moos mosaik muehle " +
  "muschel nadel nebel nelke nest nuss oase ofen orgel otter palme papier pfanne pferd pinsel planet pilz pudel " +
  "quelle rabe radio rakete regen reh riese ring rose rubin ruder saat salat sand sattel schaf schal schiff schnee " +
  "schule segel sessel sonne spatz spiegel stern stiefel strand tafel tanne tasse teich teller tiger tisch tomate " +
  "torte traube tulpe turm uhr ufer vase vogel wagen wald wal wasser welle wiese wind wolke wurzel zebra zelt " +
  "ziege zimt zitrone zucker zwerg"
).split(" ");

const DEMO_WORDS: Record<Wordlist, string[]> = { en: WORDS_EN, de: WORDS_DE };

/** Uniform random integer in [0, max). */
function randomInt(max: number): number {
  if (max <= 0) throw new Error("max must be positive");
  const limit = Math.floor(0x1_0000_0000 / max) * max;
  const buf = new Uint32Array(1);
  for (;;) {
    crypto.getRandomValues(buf);
    const value = buf[0] ?? 0;
    if (value < limit) return value % max;
  }
}

function pick<T>(list: T[]): T {
  const value = list[randomInt(list.length)];
  if (value === undefined) throw new Error("empty list");
  return value;
}

function shuffle<T>(arr: T[]): T[] {
  for (let i = arr.length - 1; i > 0; i--) {
    const j = randomInt(i + 1);
    const tmp = arr[i] as T;
    arr[i] = arr[j] as T;
    arr[j] = tmp;
  }
  return arr;
}

const isLetter = (c: string) => /^[A-Za-z]$/.test(c);

/** Throws `Error("invalid_input:<code>")` like the core's checks. */
function invalid(code: string): never {
  throw new Error(`invalid_input:${code}`);
}

function generatePassword(opts: GeneratorOptions): string {
  const result = passwordPlan(opts);
  if (!result.ok) invalid(result.issue);
  const { plan } = result;
  const chars: string[] = [];
  for (const cls of plan.classes) {
    for (let i = 0; i < cls.min; i++) chars.push(pick(cls.chars));
  }
  if (plan.extraLetter) chars.push(pick(plan.letters));
  while (chars.length < opts.length) chars.push(pick(plan.all));
  shuffle(chars);
  if (opts.startWithLetter && !isLetter(chars[0] ?? "")) {
    // Swap a uniformly chosen letter to the front (as the core does).
    const positions = chars.flatMap((c, i) => (isLetter(c) ? [i] : []));
    const pos = pick(positions);
    [chars[0], chars[pos]] = [chars[pos] as string, chars[0] as string];
  }
  return chars.join("");
}

function generatePassphrase(opts: GeneratorOptions): string {
  const issue = passphraseIssue(opts);
  if (issue) invalid(issue);
  const list = DEMO_WORDS[opts.wordlist] ?? WORDS_EN;
  const words = Array.from({ length: opts.words }, () => {
    const word = pick(list);
    return opts.capitalize ? word.charAt(0).toUpperCase() + word.slice(1) : word;
  });
  if (opts.includeNumber) {
    const idx = randomInt(words.length);
    words[idx] = `${words[idx] ?? ""}${randomInt(10)}`;
  }
  return words.join(opts.separator);
}

export function generate(opts: GeneratorOptions): string {
  return opts.kind === "passphrase" ? generatePassphrase(opts) : generatePassword(opts);
}

/** The core's `GeneratorOptions::normalized`: checked, password symbol set / exclude without duplicates. */
export function normalizeOptions(opts: GeneratorOptions): GeneratorOptions {
  if (opts.kind === "passphrase") {
    const issue = passphraseIssue(opts);
    if (issue) invalid(issue);
  } else {
    const result = passwordPlan(opts);
    if (!result.ok) invalid(result.issue);
  }
  return normalizedOptions(opts);
}

/** The core's `validate_preset_name`: trimmed, 1..=40 characters. */
export function validatePresetName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) invalid("preset_name_required");
  if (Array.from(trimmed).length > PRESET_NAME_MAX_CHARS) invalid("preset_name_too_long");
  return trimmed;
}

const D = DEFAULT_GENERATOR_OPTIONS;

/** The core's `builtin_presets()` (names are English fallbacks; the UI translates by id). */
export function builtinPresets(): GeneratorPreset[] {
  return [
    { id: "standard", name: "Standard", options: { ...D } },
    { id: "alnum", name: "Letters and digits only", options: { ...D, symbols: false, minSymbols: 0 } },
    {
      id: "pin",
      name: "PIN",
      options: {
        ...D,
        length: 6,
        uppercase: false,
        lowercase: false,
        symbols: false,
        minUppercase: 0,
        minLowercase: 0,
        minDigits: 0,
        minSymbols: 0,
      },
    },
    {
      id: "long",
      name: "Long and strong",
      options: { ...D, length: 32, minUppercase: 2, minLowercase: 2, minDigits: 2, minSymbols: 2 },
    },
    { id: "passphrase", name: "Passphrase", options: { ...D, kind: "passphrase" } },
  ];
}
