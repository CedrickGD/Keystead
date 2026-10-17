import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Menu as MenuIcon } from "lucide-react";
import { ApiError, api, events, pickAttachmentFile, pickSaveFile, subscribeEffect } from "../../lib/api";
import type { AttachmentListEntry, Folder, ItemListEntry, ItemType, VaultItem } from "../../lib/types";
import { cloneItem, fileNameOf, itemsEqual, localPrefs, newItem, searchTerms } from "../../lib/utils";
import { hasModalLayer, useBackHandler, useEscapeLayer } from "../../lib/layers";
import { isPhonePlatform, phoneBackStep } from "../../lib/phone";
import { useT, type MessageKey } from "../../i18n";
import { useApp, useCopy, useCopySecret } from "../../state/app";
import { useToast } from "../../components/Toasts";
import { useConfirm } from "../../components/Confirm";
import { Sidebar } from "./Sidebar";
import { ItemList } from "./ItemList";
import { ItemView } from "./ItemView";
import { ItemEditor } from "./ItemEditor";
import { StartPanel } from "./StartPanel";
import { countItems, filterItems, hasTag, retagTags, sameTag, tagKey, tagList, visibleItems, type Filter, type SortKey, type View } from "./model";
import { GeneratorPage } from "../GeneratorPage";
import { HealthPage } from "../HealthPage";
import { SettingsPage } from "../settings/SettingsPage";
import { ImportDialog, ImportLauncherContext } from "../../components/import/ImportDialog";
import { useFileDropTarget } from "../../components/import/FileDrop";
import type { ImportRequest } from "../../components/import/ImportFlow";
import { clearStartSection, peekStartSection } from "../../lib/startView";

// The item list holds redacted entries (`ItemListEntry`, no secrets). Only
// the editor gets a full item (`get_item_for_edit`), in `editing`, which is
// dropped again after save / cancel.
interface EditState {
  draft: VaultItem;
  /** Snapshot to detect unsaved changes. */
  baseline: VaultItem;
  isNew: boolean;
  showErrors: boolean;
}

const ACTIVITY_THROTTLE_MS = 30_000;
const ACTIVITY_EVENTS = ["mousemove", "pointerdown", "keydown", "wheel"] as const;

const folderCollator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const byName = (a: Folder, b: Folder) => folderCollator.compare(a.name, b.name);

export function MainScreen() {
  const { t, tp, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const copy = useCopy();
  const copySecret = useCopySecret();
  const { vault, lock, showUnlock, info, phoneLayout } = useApp();
  // The phone: nothing in the vault changes here (no write command exists
  // there), and the desktop's file dialogs are missing. Every write action
  // below returns early on `readOnly` as well – the UI hides them anyway.
  const readOnly = info.readOnly;
  const mobile = isPhonePlatform(info.platform);
  // Via a ref: `reload` must not change (and refetch) with every context update.
  const showUnlockRef = useRef(showUnlock);
  showUnlockRef.current = showUnlock;

  const [items, setItems] = useState<ItemListEntry[] | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [view, setView] = useState<View>(() => {
    // E.g. the setup wizard's "Anleitung" for the browser extension.
    const section = peekStartSection();
    return section ? { kind: "settings", section } : { kind: "vault", filter: { kind: "all" } };
  });
  useEffect(() => clearStartSection(), []);
  // For effects and async handlers that must see the current view without re-running on every change.
  const viewRef = useRef(view);
  viewRef.current = view;

  // The import dialog: opened from the start panel / settings, or by a file
  // dropped onto the window (`request` = that file).
  const [importDialog, setImportDialog] = useState<{ request: ImportRequest | null } | null>(null);
  const importSeq = useRef(0);
  const openImport = useCallback((path?: string, others = 0) => {
    importSeq.current += 1;
    setImportDialog({ request: path ? { path, seq: importSeq.current, others } : null });
  }, []);
  useFileDropTarget((path, others) => openImport(path, others), !readOnly);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Phone layout: the sidebar is a drawer (menu button in the list header
  // and the page bar); Escape and the back gesture close it like a menu.
  const [drawerOpen, setDrawerOpen] = useState(false);
  const drawerShown = phoneLayout && drawerOpen;
  useEscapeLayer(() => setDrawerOpen(false), drawerShown);
  useEffect(() => {
    if (!phoneLayout) setDrawerOpen(false);
  }, [phoneLayout]);
  const [editing, setEditing] = useState<EditState | null>(null);
  // Bumped whenever the detail pane moves on (another item, none, another
  // view): a `get_item_for_edit` answer for an older request is dropped
  // instead of opening the editor.
  const editRequest = useRef(0);
  useEffect(() => {
    editRequest.current += 1;
  }, [selectedId, view]);
  const [saving, setSaving] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSortState] = useState<SortKey>(() => (localPrefs.get("sort") === "updated" ? "updated" : "name"));
  const searchRef = useRef<HTMLInputElement>(null);
  const detailRef = useRef<HTMLElement>(null);

  const setSort = (next: SortKey) => {
    setSortState(next);
    localPrefs.set("sort", next);
  };

  // ------------------------------------------------------------------ data

  const reload = useCallback(async () => {
    try {
      const [loadedItems, loadedFolders] = await Promise.all([api.listItems(), api.listFolders()]);
      setItems(loadedItems);
      setFolders([...loadedFolders].sort(byName));
    } catch (err) {
      if (err instanceof ApiError && err.code === "locked") {
        // Never keep showing the items of a vault that is no longer open,
        // even if no `vault://locked` event arrives.
        showUnlockRef.current();
        return;
      }
      toast.error(errorText(err));
      setItems((current) => current ?? []);
    }
  }, [errorText, toast]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => subscribeEffect(events.onVaultChanged(() => void reload())), [reload]);

  // Reset the auto-lock timer on user activity (at most every 30 s). Only
  // input to the focused, visible window counts: WebView2 and WebKitGTK also
  // deliver mousemove to a background window the pointer merely passes over.
  useEffect(() => {
    let last = 0;
    const onActivity = () => {
      if (!document.hasFocus() || document.visibilityState !== "visible") return;
      const now = Date.now();
      if (now - last < ACTIVITY_THROTTLE_MS) return;
      last = now;
      api.touchActivity().catch(() => undefined);
    };
    for (const type of ACTIVITY_EVENTS) window.addEventListener(type, onActivity, { passive: true });
    return () => {
      for (const type of ACTIVITY_EVENTS) window.removeEventListener(type, onActivity);
    };
  }, []);

  // ------------------------------------------------------------- derived

  const allItems = useMemo(() => items ?? [], [items]);
  const counts = useMemo(() => countItems(allItems), [allItems]);
  const tags = useMemo(() => tagList(counts), [counts]);
  const tagNames = useMemo(() => tags.map((x) => x.tag), [tags]);
  const filter = useMemo<Filter>(() => (view.kind === "vault" ? view.filter : { kind: "all" }), [view]);
  const terms = useMemo(() => searchTerms(query), [query]);
  const visible = useMemo(() => visibleItems(allItems, filter, terms, sort), [allItems, filter, terms, sort]);
  const totalInFilter = useMemo(() => filterItems(allItems, filter).length, [allItems, filter]);
  const selected = selectedId ? allItems.find((i) => i.id === selectedId) ?? null : null;
  const dirty = editing ? !itemsEqual(editing.draft, editing.baseline) : false;
  /** The spelling the sidebar shows for `tag` (any spelling of it); `tag` itself if no item outside the trash has it. */
  const shownTag = (tag: string) => counts.tags.get(tagKey(tag))?.tag ?? tag;

  const filterTitle = (f: Filter): string => {
    switch (f.kind) {
      case "all":
        return t("nav.all");
      case "favorites":
        return t("nav.favorites");
      case "trash":
        return t("nav.trash");
      case "type":
        return t(`type.${f.type}.plural` as MessageKey);
      case "folder":
        return folders.find((x) => x.id === f.id)?.name ?? t("nav.folders");
      case "tag":
        return shownTag(f.tag);
    }
  };

  // A tag exists only on its items: when the last one loses it (edit, trash,
  // rename, delete, a change from elsewhere) the filter falls back to all
  // items, like a deleted folder. Only item changes can make a tag vanish,
  // so this runs on those alone (the view is read through the ref).
  useEffect(() => {
    if (!items) return;
    const current = viewRef.current;
    if (current.kind === "vault" && current.filter.kind === "tag" && !counts.tags.has(tagKey(current.filter.tag))) {
      setView({ kind: "vault", filter: { kind: "all" } });
    }
  }, [items, counts]);

  // ------------------------------------------------------- edit guarding

  /** Runs `action` after making sure no unsaved edits get lost. */
  const guard = useCallback(
    async (action: () => void) => {
      if (editing && dirty) {
        const ok = await confirm({
          title: t("edit.discardTitle"),
          message: t("edit.discardText"),
          confirmLabel: t("edit.discard"),
          cancelLabel: t("edit.keepEditing"),
          tone: "warning",
        });
        if (!ok) return;
      }
      setEditing(null);
      action();
    },
    [confirm, dirty, editing, t],
  );

  const navigate = (next: View) => {
    // Phone: the drawer closes on every choice, and a new list starts without a detail on top.
    setDrawerOpen(false);
    void guard(() => {
      setView(next);
      if (next.kind === "vault") {
        const stillVisible = selectedId && visibleItems(allItems, next.filter, terms, sort).some((i) => i.id === selectedId);
        if (!stillVisible || phoneLayout) setSelectedId(null);
      }
    });
  };

  const select = (id: string) => {
    if (id === selectedId && !editing) return;
    void guard(() => setSelectedId(id));
  };

  const focusDetail = () => {
    requestAnimationFrame(() => {
      const root = detailRef.current;
      const target =
        root?.querySelector<HTMLElement>("[data-detail-primary]") ??
        root?.querySelector<HTMLElement>(".fieldrow-actions button") ??
        root;
      target?.focus();
    });
  };

  const startNew = (type: ItemType) => {
    if (readOnly) return;
    void guard(() => {
      const folderId = filter.kind === "folder" ? filter.id : null;
      const draft = newItem(type, folderId);
      if (filter.kind === "favorites") draft.favorite = true;
      if (filter.kind === "tag") draft.tags = [shownTag(filter.tag)];
      if (view.kind !== "vault" || filter.kind === "trash") setView({ kind: "vault", filter: { kind: "all" } });
      setEditing({ draft, baseline: cloneItem(draft), isNew: true, showErrors: false });
      setSelectedId(null);
    });
  };

  /** Loads the full item (with its secrets) for the editor. */
  const startEdit = async (item: ItemListEntry) => {
    if (readOnly) return;
    const request = ++editRequest.current;
    try {
      const draft = await api.getItemForEdit(item.id);
      // Another item was selected, or the view changed, meanwhile.
      if (request !== editRequest.current) return;
      if (draft.type === "login" && draft.login && draft.login.uris.length === 0) {
        draft.login.uris = [{ uri: "", match: "domain" }];
      }
      setEditing({ draft, baseline: cloneItem(draft), isNew: false, showErrors: false });
    } catch (err) {
      if (request !== editRequest.current) return;
      if (err instanceof ApiError && err.code === "locked") {
        showUnlockRef.current();
        return;
      }
      toast.error(errorText(err));
      if (err instanceof ApiError && err.code === "not_found") void reload();
    }
  };

  const cancelEdit = () => void guard(() => undefined);

  const upsertLocal = (item: ItemListEntry) =>
    setItems((list) => {
      const current = list ?? [];
      return current.some((i) => i.id === item.id) ? current.map((i) => (i.id === item.id ? item : i)) : [...current, item];
    });

  const saveEdit = async () => {
    if (!editing || saving) return;
    if (!editing.draft.name.trim()) {
      setEditing({ ...editing, showErrors: true });
      return;
    }
    // Drop empty website rows and unnamed, empty custom fields before saving.
    const draft = cloneItem(editing.draft);
    draft.name = draft.name.trim();
    if (draft.login) draft.login.uris = draft.login.uris.filter((u) => u.uri.trim()).map((u) => ({ ...u, uri: u.uri.trim() }));
    draft.fields = draft.fields.filter((f) => f.name.trim() || f.value.trim());
    draft.tags = draft.tags.map((tag) => tag.trim()).filter(Boolean);
    setSaving(true);
    try {
      const saved = await api.saveItem(draft);
      upsertLocal(saved);
      setSelectedId(saved.id);
      setEditing(null);
      toast.success(editing.isNew ? t("item.created_toast") : t("item.saved_toast"));
    } catch (err) {
      toast.error(errorText(err));
      if (err instanceof ApiError && err.code === "conflict") void reload();
    } finally {
      setSaving(false);
    }
  };

  // ------------------------------------------------------- item actions

  const trash = async (item: ItemListEntry) => {
    if (readOnly) return;
    try {
      await api.trashItem(item.id);
      upsertLocal({ ...item, deletedAt: Date.now() });
      setEditing(null);
      if (selectedId === item.id) setSelectedId(null);
      toast.show({
        kind: "success",
        message: t("trash.moved", { name: item.name }),
        action: {
          label: t("common.undo"),
          onClick: () => {
            api
              .restoreItem(item.id)
              .then(() => {
                upsertLocal({ ...item, deletedAt: null });
                setSelectedId(item.id);
              })
              .catch((err: unknown) => toast.error(errorText(err)));
          },
        },
      });
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const restore = async (item: ItemListEntry) => {
    if (readOnly) return;
    try {
      await api.restoreItem(item.id);
      upsertLocal({ ...item, deletedAt: null });
      toast.success(t("trash.restored", { name: item.name }));
      if (filter.kind === "trash") setSelectedId(null);
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const deleteForever = async (item: ItemListEntry) => {
    if (readOnly) return;
    const ok = await confirm({
      title: t("trash.deleteForeverTitle"),
      message: t("trash.deleteForeverText", { name: item.name }),
      confirmLabel: t("trash.deleteForever"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      await api.deleteItem(item.id);
      setItems((list) => (list ?? []).filter((i) => i.id !== item.id));
      if (selectedId === item.id) setSelectedId(null);
      toast.success(t("trash.deleted"));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const emptyTrash = async () => {
    if (readOnly) return;
    const ok = await confirm({
      title: t("trash.emptyTitle"),
      message: tp("trash.emptyText", counts.trash),
      confirmLabel: t("trash.empty"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      const removed = await api.emptyTrash();
      setItems((list) => (list ?? []).filter((i) => i.deletedAt === null));
      setSelectedId(null);
      toast.success(tp("trash.emptied", removed));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const toggleFavorite = async (item: ItemListEntry) => {
    if (readOnly) return;
    try {
      // Not `saveItem`: the list entry has no secrets to send back.
      const saved = await api.setFavorite(item.id, !item.favorite);
      upsertLocal(saved);
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  // --------------------------------------------------------- attachments
  // Files act on the saved item (never through the editor): the backend reads
  // the picked file and writes the saved one itself, the page only passes
  // paths from the native dialogs. The answer is the item's new entry.

  const [attaching, setAttaching] = useState(false);

  /** `locked` → unlock screen like every other command; `not_found` → the list is stale: refetch. */
  const reportItemError = (err: unknown) => {
    if (err instanceof ApiError && err.code === "locked") {
      showUnlockRef.current();
      return;
    }
    toast.error(errorText(err));
    if (err instanceof ApiError && (err.code === "not_found" || err.code === "conflict")) void reload();
  };

  const addAttachment = async (item: ItemListEntry) => {
    if (attaching || readOnly) return;
    let path: string | null;
    try {
      path = await pickAttachmentFile(t("attachments.pickTitle"));
    } catch (err) {
      toast.error(errorText(err));
      return;
    }
    if (!path) return;
    setAttaching(true);
    try {
      const saved = await api.addAttachment(item.id, path);
      upsertLocal(saved);
      toast.success(t("attachments.added", { name: fileNameOf(path) }));
    } catch (err) {
      reportItemError(err);
    } finally {
      setAttaching(false);
    }
  };

  const saveAttachment = async (attachment: AttachmentListEntry) => {
    if (mobile) return;
    let path: string | null;
    try {
      path = await pickSaveFile({ title: t("attachments.saveTitle"), defaultPath: attachment.name });
    } catch (err) {
      toast.error(errorText(err));
      return;
    }
    if (!path) return;
    try {
      await api.saveAttachment(attachment.id, path);
      // The backend remembers the path like an export: "Im Ordner anzeigen" works.
      toast.show({
        kind: "success",
        message: t("attachments.saved", { path: fileNameOf(path) }),
        duration: 8000,
        action: {
          label: t("export.showInFolder"),
          onClick: () => {
            api.showExport().catch((err: unknown) => toast.error(errorText(err)));
          },
        },
      });
    } catch (err) {
      reportItemError(err);
    }
  };

  const removeAttachment = async (attachment: AttachmentListEntry) => {
    if (readOnly) return;
    const ok = await confirm({
      title: t("attachments.removeTitle"),
      message: t("attachments.removeText", { name: attachment.name }),
      confirmLabel: t("attachments.remove"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      const saved = await api.removeAttachment(attachment.id);
      upsertLocal(saved);
      toast.success(t("attachments.removed", { name: attachment.name }));
    } catch (err) {
      reportItemError(err);
    }
  };

  // ------------------------------------------------------------- folders

  const createFolder = async (name: string) => {
    if (readOnly) return;
    try {
      const folder = await api.saveFolder({ id: "", name });
      setFolders((list) => [...list, folder].sort(byName));
      toast.success(t("folder.created", { name: folder.name }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const renameFolder = async (folder: Folder, name: string) => {
    if (readOnly) return;
    try {
      const saved = await api.saveFolder({ ...folder, name });
      setFolders((list) => list.map((f) => (f.id === saved.id ? saved : f)).sort(byName));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const deleteFolder = async (folder: Folder) => {
    if (readOnly) return;
    const ok = await confirm({
      title: t("folder.deleteTitle", { name: folder.name }),
      message: t("folder.deleteText"),
      confirmLabel: t("folder.delete"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      await api.deleteFolder(folder.id);
      setFolders((list) => list.filter((f) => f.id !== folder.id));
      setItems((list) => (list ?? []).map((i) => (i.folderId === folder.id ? { ...i, folderId: null } : i)));
      if (view.kind === "vault" && view.filter.kind === "folder" && view.filter.id === folder.id) {
        setView({ kind: "vault", filter: { kind: "all" } });
      }
      toast.success(t("folder.deleted", { name: folder.name }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  // ---------------------------------------------------------------- tags
  // Both commands send no event; the list is refetched afterwards.

  /**
   * A vault-wide tag change reaches the item open in the editor too – draft
   * and baseline alike, so the dirty state stays right and saving the draft
   * cannot bring the old tag back (`save_item` would store it again).
   */
  const retagEditing = (from: string, to: string | null) =>
    setEditing((current) =>
      current
        ? {
            ...current,
            draft: { ...current.draft, tags: retagTags(current.draft.tags, from, to) },
            baseline: { ...current.baseline, tags: retagTags(current.baseline.tags, from, to) },
          }
        : current,
    );

  const renameTag = async (from: string, to: string) => {
    if (readOnly) return;
    try {
      const changed = await api.renameTag(from, to);
      retagEditing(from, to);
      // Follow the tag. Should no item end up with it (merged into another
      // one), the refetch below falls back to all items.
      const current = viewRef.current;
      if (current.kind === "vault" && current.filter.kind === "tag" && sameTag(current.filter.tag, from)) {
        setView({ kind: "vault", filter: { kind: "tag", tag: to } });
      }
      await reload();
      toast.success(tp("tags.renamed", changed, { from, to }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const deleteTag = async (tag: string) => {
    if (readOnly) return;
    // Like the backend: every spelling, the trash included.
    const affected = allItems.filter((i) => hasTag(i.tags, tag)).length;
    const ok = await confirm({
      title: t("tags.deleteTitle", { name: tag }),
      message: tp("tags.deleteText", affected),
      confirmLabel: t("tags.delete"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      const changed = await api.deleteTag(tag);
      retagEditing(tag, null);
      await reload();
      toast.success(tp("tags.deleted", changed, { name: tag }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  // ------------------------------------------------------------ shortcuts

  const shortcuts = useRef<(e: KeyboardEvent) => void>(() => undefined);
  shortcuts.current = (e: KeyboardEvent) => {
    if (hasModalLayer()) return;
    const mod = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (editing && mod && (key === "s" || key === "enter")) {
      e.preventDefault();
      void saveEdit();
    } else if (mod && !e.shiftKey && !e.altKey && key === "f") {
      e.preventDefault();
      if (view.kind !== "vault") {
        void guard(() => {
          setView({ kind: "vault", filter: { kind: "all" } });
          requestAnimationFrame(() => searchRef.current?.select());
        });
      } else {
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    } else if (mod && !e.shiftKey && key === "n") {
      e.preventDefault();
      startNew(filter.kind === "type" ? filter.type : "login");
    } else if (mod && !e.shiftKey && key === "l") {
      e.preventDefault();
      void lock();
    } else if (mod && e.shiftKey && key === "c") {
      if (editing || !selected?.login?.hasPassword) return;
      e.preventDefault();
      void copySecret(selected.id, "password", t("field.password"));
    } else if (mod && !e.shiftKey && key === "b") {
      e.preventDefault();
      const username = selected?.login?.username || selected?.identity?.username;
      if (!editing && username) void copy(username, { label: t("field.username"), sensitive: false });
    } else if (e.key === "Escape" && !e.defaultPrevented) {
      if (editing) {
        e.preventDefault();
        cancelEdit();
      } else if (query && document.activeElement === searchRef.current) {
        setQuery("");
      } else if (selectedId && view.kind === "vault" && !(document.activeElement instanceof HTMLInputElement)) {
        // Back to the overview (quick actions & shortcuts) in the detail pane.
        e.preventDefault();
        setSelectedId(null);
      }
    }
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => shortcuts.current(e);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // The phone's back gesture once no dialog, menu or drawer is open (those
  // close first, lib/layers.ts): editor → detail → search → all items →
  // Android (the app moves to the background).
  useBackHandler(() => {
    const step = phoneBackStep({
      editing: editing !== null,
      detailOpen: view.kind === "vault" && selected !== null,
      searching: query !== "",
      page: view.kind !== "vault",
      filtered: view.kind === "vault" && view.filter.kind !== "all",
    });
    switch (step) {
      case "editor":
        cancelEdit();
        return true;
      case "detail":
        setSelectedId(null);
        return true;
      case "search":
        setQuery("");
        return true;
      case "home":
        navigate({ kind: "vault", filter: { kind: "all" } });
        return true;
      default:
        return false;
    }
  }, phoneLayout);

  // --------------------------------------------------------------- render

  const renderDetail = () => {
    if (editing) {
      return (
        <ItemEditor
          key={editing.draft.id || "new"}
          draft={editing.draft}
          folders={folders}
          knownTags={tagNames}
          isNew={editing.isNew}
          saving={saving}
          showErrors={editing.showErrors}
          onChange={(draft) => setEditing((current) => (current ? { ...current, draft } : current))}
          onSave={() => void saveEdit()}
          onCancel={cancelEdit}
          onDelete={() => {
            const original = allItems.find((i) => i.id === editing.draft.id);
            if (original) void trash(original);
          }}
        />
      );
    }
    if (selected) {
      return (
        <ItemView
          key={selected.id}
          item={selected}
          folders={folders}
          onEdit={() => void startEdit(selected)}
          onTrash={() => void trash(selected)}
          onRestore={() => void restore(selected)}
          onDeleteForever={() => void deleteForever(selected)}
          onToggleFavorite={() => void toggleFavorite(selected)}
          onOpenTag={(tag) => {
            // Only a tag some item outside the trash carries has a filter
            // (and a sidebar row); its shown spelling names the filter.
            const known = counts.tags.get(tagKey(tag));
            if (known) navigate({ kind: "vault", filter: { kind: "tag", tag: known.tag } });
          }}
          attaching={attaching}
          onAddAttachment={() => void addAttachment(selected)}
          onSaveAttachment={(attachment) => void saveAttachment(attachment)}
          onRemoveAttachment={(attachment) => void removeAttachment(attachment)}
          readOnly={readOnly}
          canSaveAttachments={!mobile}
          onBack={phoneLayout ? () => setSelectedId(null) : undefined}
        />
      );
    }
    // Phone: the detail is stacked over the list only while an item is open.
    if (!items || phoneLayout) return null;
    return (
      <StartPanel
        empty={counts.all === 0}
        onNewLogin={() => startNew("login")}
        onGenerator={() => navigate({ kind: "generator" })}
        onSettings={(section) => navigate({ kind: "settings", section })}
      />
    );
  };

  const detailOpen = view.kind === "vault" && (editing !== null || selected !== null);
  const openDrawer = () => setDrawerOpen(true);

  let page: ReactNode = null;
  let pageTitle = "";
  if (view.kind === "generator") {
    page = <GeneratorPage />;
    pageTitle = t("nav.generator");
  } else if (view.kind === "health") {
    page = (
      <HealthPage
        items={allItems}
        onOpenItem={(id) => {
          setView({ kind: "vault", filter: { kind: "all" } });
          setQuery("");
          setSelectedId(id);
        }}
      />
    );
    pageTitle = t("nav.health");
  } else if (view.kind === "settings") {
    page = <SettingsPage initialSection={view.section} items={allItems} />;
    pageTitle = t("nav.settings");
  }

  return (
    <ImportLauncherContext.Provider value={readOnly ? null : openImport}>
      <div className={`shell ${drawerShown ? "drawer-open" : ""}`}>
        <Sidebar
          vaultName={vault?.name ?? ""}
          view={view}
          counts={counts}
          folders={folders}
          tags={tags}
          readOnly={readOnly}
          drawerOpen={drawerShown}
          onNavigate={navigate}
          onLock={() => void lock()}
          onCreateFolder={(name) => void createFolder(name)}
          onRenameFolder={(folder, name) => void renameFolder(folder, name)}
          onDeleteFolder={(folder) => void deleteFolder(folder)}
          onRenameTag={(tag, name) => void renameTag(tag, name)}
          onDeleteTag={(tag) => void deleteTag(tag)}
        />
        {drawerShown && <div className="drawer-scrim" onClick={() => setDrawerOpen(false)} aria-hidden />}
        {view.kind === "vault" && (
          <div className={`vault-view ${detailOpen ? "detail-open" : ""}`}>
            <ItemList
              ref={searchRef}
              title={filterTitle(filter)}
              filter={filter}
              items={visible}
              loading={items === null}
              totalInFilter={totalInFilter}
              selectedId={editing && !editing.isNew ? editing.draft.id : selectedId}
              onSelect={select}
              onOpen={focusDetail}
              query={query}
              onQuery={setQuery}
              terms={terms}
              sort={sort}
              onSort={setSort}
              onNew={startNew}
              onEmptyTrash={() => void emptyTrash()}
              readOnly={readOnly}
              onMenu={phoneLayout ? openDrawer : undefined}
            />
            <main className="detail-pane" ref={detailRef} aria-label={t("detail.label")}>
              {renderDetail()}
            </main>
          </div>
        )}
        {page &&
          (phoneLayout ? (
            // Phone: the pages get a bar with the menu button (the drawer holds the navigation).
            <div className="phone-page">
              <header className="phone-bar">
                <button type="button" className="icon-btn" onClick={openDrawer} aria-label={t("nav.openMenu")} title={t("nav.openMenu")}>
                  <MenuIcon />
                </button>
                <span className="phone-bar-title truncate">{pageTitle}</span>
              </header>
              {page}
            </div>
          ) : (
            page
          ))}
        {importDialog && <ImportDialog request={importDialog.request} onClose={() => setImportDialog(null)} />}
      </div>
    </ImportLauncherContext.Provider>
  );
}
