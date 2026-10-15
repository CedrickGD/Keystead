import type { ItemListEntry, ItemType } from "../../lib/types";
import { matchesSearch } from "../../lib/utils";

export type Filter =
  | { kind: "all" }
  | { kind: "favorites" }
  | { kind: "type"; type: ItemType }
  | { kind: "folder"; id: string }
  /** Items carrying `tag` in any spelling ("Bank" and "bank" are one tag, as in the core). */
  | { kind: "tag"; tag: string }
  | { kind: "trash" };

export type View =
  | { kind: "vault"; filter: Filter }
  | { kind: "generator" }
  | { kind: "health" }
  | { kind: "settings"; section?: string };

export type SortKey = "name" | "updated";

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

/** The key under which all spellings of a tag are one tag, like `tag_eq` in the core. */
export function tagKey(tag: string): string {
  return tag.toLowerCase();
}

export function sameTag(a: string, b: string): boolean {
  return tagKey(a) === tagKey(b);
}

export function hasTag(tags: string[], tag: string): boolean {
  return tags.some((t) => sameTag(t, tag));
}

/**
 * `rename_tag` (`to` a string) / `delete_tag` (`to` null) applied to one
 * item's tags, like `retag_item` in the core: `from` is matched in any
 * spelling; a list that already carries `to` in another spelling keeps that
 * one and only loses `from`, otherwise `from` is replaced in place. Returns
 * the same array when the list has no `from`.
 */
export function retagTags(tags: string[], from: string, to: string | null): string[] {
  if (!hasTag(tags, from)) return tags;
  const merge = to !== null && tags.some((t) => !sameTag(t, from) && sameTag(t, to));
  const next: string[] = [];
  let replaced = false;
  for (const tag of tags) {
    if (!sameTag(tag, from)) next.push(tag);
    else if (to !== null && !merge && !replaced) {
      next.push(to);
      replaced = true;
    }
  }
  return next;
}

export function sameFilter(a: Filter, b: Filter): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "type" && b.kind === "type") return a.type === b.type;
  if (a.kind === "folder" && b.kind === "folder") return a.id === b.id;
  if (a.kind === "tag" && b.kind === "tag") return sameTag(a.tag, b.tag);
  return true;
}

export function filterItems(items: ItemListEntry[], filter: Filter): ItemListEntry[] {
  switch (filter.kind) {
    case "all":
      return items.filter((i) => i.deletedAt === null);
    case "favorites":
      return items.filter((i) => i.deletedAt === null && i.favorite);
    case "type":
      return items.filter((i) => i.deletedAt === null && i.type === filter.type);
    case "folder":
      return items.filter((i) => i.deletedAt === null && i.folderId === filter.id);
    case "tag":
      return items.filter((i) => i.deletedAt === null && hasTag(i.tags, filter.tag));
    case "trash":
      return items.filter((i) => i.deletedAt !== null);
  }
}

export function visibleItems(items: ItemListEntry[], filter: Filter, terms: string[], sort: SortKey): ItemListEntry[] {
  const list = filterItems(items, filter).filter((item) => matchesSearch(item, terms));
  if (filter.kind === "trash") return list.sort((a, b) => (b.deletedAt ?? 0) - (a.deletedAt ?? 0));
  if (sort === "updated") return list.sort((a, b) => b.updatedAt - a.updatedAt || collator.compare(a.name, b.name));
  return list.sort((a, b) => collator.compare(a.name, b.name));
}

export interface Counts {
  all: number;
  favorites: number;
  login: number;
  card: number;
  identity: number;
  note: number;
  trash: number;
  folders: Record<string, number>;
  /**
   * Items per tag without the trash, keyed by `tagKey`: the spellings of a
   * tag are one entry, as the core treats them. A Map: tags are free text
   * ("constructor" is a valid tag).
   */
  tags: Map<string, TagCount>;
}

export interface TagCount {
  /** The spelling shown: the most frequent one, on a tie the first in list order. */
  tag: string;
  count: number;
}

export function countItems(items: ItemListEntry[]): Counts {
  const counts: Counts = { all: 0, favorites: 0, login: 0, card: 0, identity: 0, note: 0, trash: 0, folders: {}, tags: new Map() };
  // Items per spelling, grouped by tag.
  const spellings = new Map<string, Map<string, number>>();
  for (const item of items) {
    if (item.deletedAt !== null) {
      counts.trash++;
      continue;
    }
    counts.all++;
    if (item.favorite) counts.favorites++;
    counts[item.type]++;
    if (item.folderId) counts.folders[item.folderId] = (counts.folders[item.folderId] ?? 0) + 1;
    for (const tag of item.tags) {
      const bySpelling = spellings.get(tagKey(tag)) ?? new Map<string, number>();
      bySpelling.set(tag, (bySpelling.get(tag) ?? 0) + 1);
      spellings.set(tagKey(tag), bySpelling);
    }
  }
  for (const [key, bySpelling] of spellings) {
    let shown = "";
    let best = 0;
    let count = 0;
    for (const [spelling, n] of bySpelling) {
      count += n;
      if (n > best) {
        best = n;
        shown = spelling;
      }
    }
    counts.tags.set(key, { tag: shown, count });
  }
  return counts;
}

/** The vault's tags (outside the trash) for the sidebar, sorted like folders. */
export function tagList(counts: Counts): TagCount[] {
  return [...counts.tags.values()].sort((a, b) => collator.compare(a.tag, b.tag) || (a.tag < b.tag ? -1 : a.tag > b.tag ? 1 : 0));
}
