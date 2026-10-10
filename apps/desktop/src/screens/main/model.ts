import type { ItemListEntry, ItemType } from "../../lib/types";
import { matchesSearch } from "../../lib/utils";

export type Filter =
  | { kind: "all" }
  | { kind: "favorites" }
  | { kind: "type"; type: ItemType }
  | { kind: "folder"; id: string }
  /** Items carrying `tag` in exactly this spelling (the sidebar lists stored spellings). */
  | { kind: "tag"; tag: string }
  | { kind: "trash" };

export type View =
  | { kind: "vault"; filter: Filter }
  | { kind: "generator" }
  | { kind: "health" }
  | { kind: "settings"; section?: string };

export type SortKey = "name" | "updated";

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

export function sameFilter(a: Filter, b: Filter): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "type" && b.kind === "type") return a.type === b.type;
  if (a.kind === "folder" && b.kind === "folder") return a.id === b.id;
  if (a.kind === "tag" && b.kind === "tag") return a.tag === b.tag;
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
      return items.filter((i) => i.deletedAt === null && i.tags.includes(filter.tag));
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
  /** Items per tag (stored spelling), without the trash. A Map: tags are free text ("constructor" is a valid tag). */
  tags: Map<string, number>;
}

export function countItems(items: ItemListEntry[]): Counts {
  const counts: Counts = { all: 0, favorites: 0, login: 0, card: 0, identity: 0, note: 0, trash: 0, folders: {}, tags: new Map() };
  for (const item of items) {
    if (item.deletedAt !== null) {
      counts.trash++;
      continue;
    }
    counts.all++;
    if (item.favorite) counts.favorites++;
    counts[item.type]++;
    if (item.folderId) counts.folders[item.folderId] = (counts.folders[item.folderId] ?? 0) + 1;
    for (const tag of item.tags) counts.tags.set(tag, (counts.tags.get(tag) ?? 0) + 1);
  }
  return counts;
}

export interface TagCount {
  tag: string;
  count: number;
}

/** The vault's tags (outside the trash) for the sidebar, sorted like folders. */
export function tagList(counts: Counts): TagCount[] {
  return [...counts.tags]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => collator.compare(a.tag, b.tag) || (a.tag < b.tag ? -1 : a.tag > b.tag ? 1 : 0));
}
