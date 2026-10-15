// Website icons of the mock backend: the demo "Privat" vault starts with a
// few stored icons (as if fetched before); a simulated background fetch
// adds the sample icons of new hosts after unlocking or saving, like the
// real fetcher would (see docs/ARCHITECTURE.md, "Website icons").

import { iconHost } from "../icons";
import type { VaultItem } from "../types";
import { SAMPLE_ICONS } from "./sampleIcons";

/** vault id → host → base64 PNG */
const stored = new Map<string, Map<string, string>>();

function hostsOf(items: VaultItem[]): Set<string> {
  const hosts = new Set<string>();
  for (const item of items) {
    if (item.deletedAt != null) continue;
    const host = iconHost(item);
    if (host) hosts.add(host);
  }
  return hosts;
}

function iconsOf(vaultId: string): Map<string, string> {
  let map = stored.get(vaultId);
  if (!map) {
    map = new Map();
    stored.set(vaultId, map);
  }
  return map;
}

/** Stores the sample icons of every host in `items` (demo start state). */
export function seedSampleIcons(vaultId: string, items: VaultItem[]): void {
  mockFetchIcons(vaultId, items);
}

/** `get_icons`: host → data URL; icons of hosts no login uses are dropped. */
export function mockGetIcons(vaultId: string, items: VaultItem[]): Record<string, string> {
  const map = iconsOf(vaultId);
  const hosts = hostsOf(items);
  const out: Record<string, string> = {};
  for (const [host, png] of [...map]) {
    if (!hosts.has(host)) map.delete(host);
    else out[host] = `data:image/png;base64,${png}`;
  }
  return out;
}

/** `clear_icons`: removes every stored icon; returns how many. */
export function mockClearIcons(vaultId: string): number {
  const map = iconsOf(vaultId);
  const count = map.size;
  map.clear();
  return count;
}

/** A simulated fetch run: stores sample icons for hosts without one. */
export function mockFetchIcons(vaultId: string, items: VaultItem[]): boolean {
  const map = iconsOf(vaultId);
  let changed = false;
  for (const host of hostsOf(items)) {
    const png = SAMPLE_ICONS[host];
    if (png && !map.has(host)) {
      map.set(host, png);
      changed = true;
    }
  }
  return changed;
}
