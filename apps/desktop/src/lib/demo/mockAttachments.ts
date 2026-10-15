// Attachments of the mock backend: a side store per vault like the mock
// icons, holding name, size and date only – the demo keeps no file bytes.
// Mirrors the rules of `keystead_core::attachments` and
// apps/desktop/src-tauri/src/attachments.rs (limits, error codes, order, the
// source/target path checks), see "Attachments" in docs/ARCHITECTURE.md. A
// "picked" file is just a path string: its size is derived from the name (one
// sample is too large on purpose), the preview of an image is a drawing made
// here, one per name.

import { ATTACHMENTS_PER_ITEM_MAX, ATTACHMENT_MAX_BYTES, type AttachmentListEntry, type VaultItem } from "../types";
import { fileNameOf } from "../utils";
import type { SampleAttachment } from "./sampleData";

const MIB = 1024 * 1024;
const NAME_MAX_CHARS = 200;
/** `unsupported:attachment_preview` above this (8 MiB, like the backend). */
const PREVIEW_MAX_BYTES = 8 * MIB;

interface MockAttachment {
  itemId: string;
  name: string;
  size: number;
  createdAt: number;
}

/** vault id → attachment id → entry */
const stored = new Map<string, Map<string, MockAttachment>>();

function fail(code: string): never {
  // Like the Tauri backend: invoke() rejects with the plain error code.
  throw code;
}

function uuid(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

function storeOf(vaultId: string): Map<string, MockAttachment> {
  let map = stored.get(vaultId);
  if (!map) {
    map = new Map();
    stored.set(vaultId, map);
  }
  return map;
}

/** Demo start state: the sample attachments of a vault. */
export function seedSampleAttachments(vaultId: string, attachments: SampleAttachment[], now: number): void {
  const map = storeOf(vaultId);
  for (const a of attachments) {
    map.set(a.id, { itemId: a.itemId, name: a.name, size: a.size, createdAt: now - a.created * 86_400_000 });
  }
}

/** The attachments of an item, oldest first (then by id), like `attachments_of` in the core. */
export function mockAttachmentsOf(vaultId: string, itemId: string): AttachmentListEntry[] {
  return [...storeOf(vaultId)]
    .filter(([, a]) => a.itemId === itemId)
    .map(([id, a]) => ({ id, name: a.name, size: a.size, createdAt: a.createdAt }))
    .sort((x, y) => x.createdAt - y.createdAt || (x.id < y.id ? -1 : x.id > y.id ? 1 : 0));
}

/**
 * `add_attachment`: the checks in the backend's order – the path and the
 * "file" first (shape and source, name, size), then the item (unknown,
 * trashed, full) and the name limits. Returns nothing; the caller answers the
 * item's new entry.
 */
export function mockAddAttachment(vaultId: string, items: VaultItem[], itemId: string, path: string, dataDir: string): void {
  if (!path.trim()) fail("invalid_input:path_required");
  if (!isDialogPath(path) || isInside(path, dataDir)) fail("invalid_input:attachment_source");
  const name = fileNameOf(path).trim();
  if (!name || /[\\/]$/.test(path)) fail("invalid_input:attachment_name_required");
  const size = mockFileSize(path);
  if (size > ATTACHMENT_MAX_BYTES) fail("unsupported:attachment_too_large");
  const item = items.find((i) => i.id === itemId);
  if (!item) fail("not_found");
  if (item.deletedAt !== null) fail("invalid_input:item_trashed");
  if (mockAttachmentsOf(vaultId, itemId).length >= ATTACHMENTS_PER_ITEM_MAX) fail("invalid_input:attachments_too_many");
  if ([...name].length > NAME_MAX_CHARS) fail("invalid_input:attachment_name_too_long");
  storeOf(vaultId).set(uuid(), { itemId, name, size, createdAt: Date.now() });
}

/** `remove_attachment`: drops the entry and answers the id of its item (`not_found` for an unknown id). */
export function mockRemoveAttachment(vaultId: string, attachmentId: string): string {
  const map = storeOf(vaultId);
  const entry = map.get(attachmentId);
  if (!entry) fail("not_found");
  map.delete(attachmentId);
  return entry.itemId;
}

/**
 * `save_attachment`: the backend refuses a target inside the vaults folder,
 * with the vault file's extension or not of the dialog's shape; everything
 * else "succeeds" (nothing is written in the demo).
 */
export function mockSaveAttachment(vaultId: string, attachmentId: string, path: string, dataDir: string): void {
  if (!path.trim()) fail("invalid_input:path_required");
  if (!storeOf(vaultId).has(attachmentId)) fail("not_found");
  if (!isDialogPath(path) || path.toLowerCase().endsWith(".keystead") || isInside(path, `${dataDir}\\vaults`)) {
    fail("invalid_input:attachment_target");
  }
}

/** A path as the native dialogs produce it: absolute (drive, UNC or POSIX root), no `.`/`..` segments. */
function isDialogPath(path: string): boolean {
  const absolute = /^([A-Za-z]:[\\/]|\\\\|\/)/.test(path);
  const dotSegment = path.split(/[\\/]/).some((segment) => segment === "." || segment === "..");
  return absolute && !dotSegment;
}

/** `path` is `dir` or lies below it (compared case-insensitively, like Windows). */
function isInside(path: string, dir: string): boolean {
  const prefix = dir.replace(/[\\/]+$/, "").toLowerCase();
  const lower = path.toLowerCase();
  return lower === prefix || lower.startsWith(`${prefix}\\`) || lower.startsWith(`${prefix}/`);
}

/** `reveal_attachment`: a data URL for an image (by name here, by magic bytes in the backend) up to 8 MiB. */
export function mockRevealAttachment(vaultId: string, attachmentId: string): string {
  const entry = storeOf(vaultId).get(attachmentId);
  if (!entry) fail("not_found");
  if (entry.size > PREVIEW_MAX_BYTES || !isImageName(entry.name)) fail("unsupported:attachment_preview");
  return previewImage(entry.name);
}

/** Entries of items that no longer exist are dropped (trash keeps its entries), like `attachments::prune` on save. */
export function mockPruneAttachments(vaultId: string, items: VaultItem[]): void {
  const ids = new Set(items.map((i) => i.id));
  const map = storeOf(vaultId);
  for (const [id, a] of [...map]) if (!ids.has(a.itemId)) map.delete(id);
}

/** `delete_vault` removes the side-car directory with it. */
export function mockDeleteAttachments(vaultId: string): void {
  stored.delete(vaultId);
}

const IMAGE_NAME = /\.(png|jpe?g|gif|webp)$/i;

function isImageName(name: string): boolean {
  return IMAGE_NAME.test(name);
}

// ---------------------------------------------------------------------------
// "Files" of the demo
// ---------------------------------------------------------------------------

/** What the mock open dialog "picks", one after the other; the video is over the 20 MiB limit on purpose. */
const SAMPLE_PICKS = [
  "C:\\Users\\Demo\\Documents\\Mietvertrag.pdf",
  "C:\\Users\\Demo\\Pictures\\Kontoauszug.png",
  "C:\\Users\\Demo\\Downloads\\Steuerbescheid-2025.pdf",
  "C:\\Users\\Demo\\Videos\\Urlaubsvideo.mp4",
];
let nextPick = 0;

/** The mock open dialog for attachments (api.ts `pickAttachmentFile`): cycles through the sample paths. */
export function mockPickAttachmentFile(): string {
  const path = SAMPLE_PICKS[nextPick % SAMPLE_PICKS.length];
  nextPick += 1;
  if (!path) fail("not_found");
  return path;
}

/** A stable pretend size for a path: videos are too large, everything else 20 KB … 3 MB by name. */
export function mockFileSize(path: string): number {
  const name = fileNameOf(path);
  if (/\.(mp4|mov|mkv|avi)$/i.test(name)) return 25 * MIB;
  let hash = 7;
  for (const ch of name) hash = (hash * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  return 20_480 + (hash % (3 * MIB - 20_480));
}

/** Rendered previews by attachment name – the backend answers each attachment's own bytes, the demo its own drawing. */
const previews = new Map<string, string>();

/** The "decrypted" image of an attachment: a drawn ID-card-like placeholder with its name, as a PNG data URL. */
function previewImage(name: string): string {
  const cached = previews.get(name);
  if (cached) return cached;
  const canvas = document.createElement("canvas");
  canvas.width = 640;
  canvas.height = 400;
  const ctx = canvas.getContext("2d");
  if (!ctx) fail("unsupported:attachment_preview");
  ctx.fillStyle = "#dfe6f2";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "#f7f9fc";
  ctx.beginPath();
  ctx.roundRect(40, 40, 560, 320, 24);
  ctx.fill();
  ctx.fillStyle = "#c9d3e3";
  ctx.beginPath();
  ctx.roundRect(72, 96, 150, 190, 12);
  ctx.fill();
  ctx.fillStyle = "#9aa8c0";
  ctx.beginPath();
  ctx.arc(147, 165, 36, 0, Math.PI * 2);
  ctx.fill();
  ctx.beginPath();
  ctx.roundRect(92, 212, 110, 60, 30);
  ctx.fill();
  ctx.fillStyle = "#b4bfd3";
  for (const [y, w] of [
    [110, 300],
    [150, 220],
    [190, 260],
    [230, 180],
    [270, 240],
  ] as const) {
    ctx.beginPath();
    ctx.roundRect(260, y, w, 16, 8);
    ctx.fill();
  }
  ctx.fillStyle = "#6d7a94";
  ctx.font = "600 20px 'Segoe UI', Inter, system-ui, sans-serif";
  ctx.fillText(name, 72, 330);
  const url = canvas.toDataURL("image/png");
  previews.set(name, url);
  return url;
}
