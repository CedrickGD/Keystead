// Password / passphrase generator for the mock backend (the real one lives in
// keystead-core). Uses crypto.getRandomValues with rejection sampling so every
// character is picked uniformly.

import type { GeneratorOptions } from "../types";

const UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER = "abcdefghijklmnopqrstuvwxyz";
const DIGITS = "0123456789";
const SYMBOLS = "!@#$%^&*";
const AMBIGUOUS = /[Il1O0]/g;

// A short list of easy-to-type English words (the real app embeds the EFF large wordlist).
const WORDS = (
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

function pick(chars: string): string {
  return chars.charAt(randomInt(chars.length));
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

export function generate(opts: GeneratorOptions): string {
  if (opts.kind === "passphrase") {
    if (opts.words < 3 || opts.words > 20) throw new Error("invalid_input:words");
    const words = Array.from({ length: opts.words }, () => {
      const word = WORDS[randomInt(WORDS.length)] ?? "word";
      return opts.capitalize ? word.charAt(0).toUpperCase() + word.slice(1) : word;
    });
    if (opts.includeNumber) {
      const idx = randomInt(words.length);
      words[idx] = `${words[idx] ?? ""}${randomInt(10)}`;
    }
    return words.join(opts.separator);
  }

  if (opts.length < 5 || opts.length > 128) throw new Error("invalid_input:length");
  const strip = (s: string) => (opts.avoidAmbiguous ? s.replace(AMBIGUOUS, "") : s);
  const upper = opts.uppercase ? strip(UPPER) : "";
  const lower = opts.lowercase ? strip(LOWER) : "";
  const digits = opts.digits ? strip(DIGITS) : "";
  const symbols = opts.symbols ? SYMBOLS : "";
  const all = upper + lower + digits + symbols;
  if (!all) throw new Error("invalid_input:no_character_set");

  const chars: string[] = [];
  if (upper) chars.push(pick(upper));
  if (lower) chars.push(pick(lower));
  for (let i = 0; digits && i < Math.max(opts.minDigits, 1); i++) chars.push(pick(digits));
  for (let i = 0; symbols && i < Math.max(opts.minSymbols, 1); i++) chars.push(pick(symbols));
  if (chars.length > opts.length) throw new Error("invalid_input:minimums_exceed_length");
  while (chars.length < opts.length) chars.push(pick(all));
  return shuffle(chars).join("");
}
