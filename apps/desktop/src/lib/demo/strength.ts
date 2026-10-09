// A deliberately simple password strength heuristic for the mock backend.
// The real app uses zxcvbn in vaultx-core; this only has to behave plausibly.

import type { Strength } from "../types";

const COMMON = [
  "password", "passwort", "123456", "12345678", "qwerty", "qwertz", "hallo", "hello", "letmein",
  "admin", "welcome", "willkommen", "sommer", "summer", "winter", "fruehling", "herbst", "iloveyou",
  "monkey", "dragon", "master", "football", "fussball", "schatz", "geheim", "secret", "test", "demo",
  "abc123", "111111", "000000", "baseball", "sunshine", "princess", "login",
];

const SEQUENCES = ["abcdefghijklmnopqrstuvwxyz", "qwertyuiop", "qwertzuiop", "asdfghjkl", "yxcvbnm", "zxcvbnm", "01234567890"];

function hasSequence(lower: string, minLen: number): boolean {
  for (const seq of SEQUENCES) {
    for (let i = 0; i + minLen <= seq.length; i++) {
      const part = seq.slice(i, i + minLen);
      if (lower.includes(part) || lower.includes([...part].reverse().join(""))) return true;
    }
  }
  return false;
}

function crackTimeDisplay(seconds: number): string {
  const minute = 60;
  const hour = minute * 60;
  const day = hour * 24;
  const month = day * 31;
  const year = month * 12;
  const century = year * 100;
  const unit = (n: number, name: string) => `${n} ${name}${n === 1 ? "" : "s"}`;
  if (seconds < 1) return "less than a second";
  if (seconds < minute) return unit(Math.round(seconds), "second");
  if (seconds < hour) return unit(Math.round(seconds / minute), "minute");
  if (seconds < day) return unit(Math.round(seconds / hour), "hour");
  if (seconds < month) return unit(Math.round(seconds / day), "day");
  if (seconds < year) return unit(Math.round(seconds / month), "month");
  if (seconds < century) return unit(Math.round(seconds / year), "year");
  return "centuries";
}

export function estimateStrength(password: string, userInputs: string[] = []): Strength {
  if (!password) {
    return { score: 0, crackTime: "less than a second", warning: "", suggestions: ["Use a few words, avoid common phrases."] };
  }
  const lower = password.toLowerCase();
  let pool = 0;
  if (/[a-z]/.test(password)) pool += 26;
  if (/[A-Z]/.test(password)) pool += 26;
  if (/[0-9]/.test(password)) pool += 10;
  if (/[^a-zA-Z0-9]/.test(password)) pool += 33;
  const unique = new Set(password).size;
  // Repeated characters add little entropy.
  const effectiveLength = Math.min(password.length, unique * 2.5);
  let bits = effectiveLength * Math.log2(Math.max(pool, 2));

  let warning = "";
  const suggestions: string[] = [];
  const dictionaryHit = [...COMMON, ...userInputs.map((s) => s.toLowerCase()).filter((s) => s.length >= 3)].find((w) =>
    lower.includes(w),
  );
  if (dictionaryHit) {
    // Replace the dictionary word's contribution with a small constant.
    bits -= dictionaryHit.length * Math.log2(Math.max(pool, 2)) - 10;
    warning = "This is similar to a commonly used password.";
    suggestions.push("Avoid common words and patterns.");
  }
  if (hasSequence(lower, 4)) {
    bits -= 12;
    warning ||= "Sequences like abc or 1234 are easy to guess.";
  }
  if (/(19|20)\d\d/.test(password)) {
    bits -= 6;
    suggestions.push("Avoid years that are associated with you.");
  }
  if (/^[0-9]+$/.test(password)) bits -= 8;
  if (password.length < 8) suggestions.push("Use a longer password.");
  bits = Math.max(0, bits);

  const score = bits < 28 ? 0 : bits < 40 ? 1 : bits < 60 ? 2 : bits < 80 ? 3 : 4;
  // Offline attack against a slow hash: ~10^4 guesses per second.
  const seconds = 2 ** Math.min(bits, 200) / 2 / 1e4;
  if (score < 3 && suggestions.length === 0) suggestions.push("Add another word or two. Uncommon words are better.");
  return { score, crackTime: crackTimeDisplay(seconds), warning, suggestions };
}
