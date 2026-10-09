import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FileUp, MousePointerClick } from "lucide-react";
import { ApiError, api, events, subscribeEffect } from "../../lib/api";
import type { Folder, ItemType, VaultItem } from "../../lib/types";
import { cloneItem, itemsEqual, localPrefs, newItem, searchTerms } from "../../lib/utils";
import { hasModalLayer } from "../../lib/layers";
import { useT, type MessageKey } from "../../i18n";
import { useApp, useCopy } from "../../state/app";
import { useToast } from "../../components/Toasts";
import { useConfirm } from "../../components/Confirm";
import { EmptyState } from "../../components/EmptyState";
import { Sidebar } from "./Sidebar";
import { ItemList } from "./ItemList";
import { ItemView } from "./ItemView";
import { ItemEditor } from "./ItemEditor";
import { countItems, filterItems, visibleItems, type Filter, type SortKey, type View } from "./model";
import { GeneratorPage } from "../GeneratorPage";
import { HealthPage } from "../HealthPage";
import { SettingsPage } from "../settings/SettingsPage";

interface EditState {
  draft: VaultItem;
  /** Snapshot to detect unsaved changes. */
  baseline: VaultItem;
  isNew: boolean;
  showErrors: boolean;
}

const ACTIVITY_THROTTLE_MS = 30_000;

const folderCollator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const byName = (a: Folder, b: Folder) => folderCollator.compare(a.name, b.name);

export function MainScreen() {
  const { t, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const copy = useCopy();
  const { vault, lock } = useApp();

  const [items, setItems] = useState<VaultItem[] | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [view, setView] = useState<View>({ kind: "vault", filter: { kind: "all" } });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editing, setEditing] = useState<EditState | null>(null);
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
      if (err instanceof ApiError && err.code === "locked") return; // the lock event handles navigation
      toast.error(errorText(err));
      setItems((current) => current ?? []);
    }
  }, [errorText, toast]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => subscribeEffect(events.onVaultChanged(() => void reload())), [reload]);

  // Reset the auto-lock timer on user activity (at most every 30 s).
  useEffect(() => {
    let last = 0;
    const onActivity = () => {
      const now = Date.now();
      if (now - last < ACTIVITY_THROTTLE_MS) return;
      last = now;
      api.touchActivity().catch(() => undefined);
    };
    window.addEventListener("mousemove", onActivity, { passive: true });
    window.addEventListener("keydown", onActivity, { passive: true });
    return () => {
      window.removeEventListener("mousemove", onActivity);
      window.removeEventListener("keydown", onActivity);
    };
  }, []);

  // ------------------------------------------------------------- derived

  const allItems = useMemo(() => items ?? [], [items]);
  const counts = useMemo(() => countItems(allItems), [allItems]);
  const filter = useMemo<Filter>(() => (view.kind === "vault" ? view.filter : { kind: "all" }), [view]);
  const terms = useMemo(() => searchTerms(query), [query]);
  const visible = useMemo(() => visibleItems(allItems, filter, terms, sort), [allItems, filter, terms, sort]);
  const totalInFilter = useMemo(() => filterItems(allItems, filter).length, [allItems, filter]);
  const selected = selectedId ? allItems.find((i) => i.id === selectedId) ?? null : null;
  const dirty = editing ? !itemsEqual(editing.draft, editing.baseline) : false;

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
    }
  };

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

  const navigate = (next: View) =>
    void guard(() => {
      setView(next);
      if (next.kind === "vault") {
        const stillVisible = selectedId && visibleItems(allItems, next.filter, terms, sort).some((i) => i.id === selectedId);
        if (!stillVisible) setSelectedId(null);
      }
    });

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

  const startNew = (type: ItemType) =>
    void guard(() => {
      const folderId = filter.kind === "folder" ? filter.id : null;
      const draft = newItem(type, folderId);
      if (filter.kind === "favorites") draft.favorite = true;
      if (view.kind !== "vault" || filter.kind === "trash") setView({ kind: "vault", filter: { kind: "all" } });
      setEditing({ draft, baseline: cloneItem(draft), isNew: true, showErrors: false });
      setSelectedId(null);
    });

  const startEdit = (item: VaultItem) => {
    const draft = cloneItem(item);
    if (draft.type === "login" && draft.login && draft.login.uris.length === 0) {
      draft.login.uris = [{ uri: "", match: "domain" }];
    }
    setEditing({ draft, baseline: cloneItem(draft), isNew: false, showErrors: false });
  };

  const cancelEdit = () => void guard(() => undefined);

  const upsertLocal = (item: VaultItem) =>
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

  const trash = async (item: VaultItem) => {
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

  const restore = async (item: VaultItem) => {
    try {
      await api.restoreItem(item.id);
      upsertLocal({ ...item, deletedAt: null });
      toast.success(t("trash.restored", { name: item.name }));
      if (filter.kind === "trash") setSelectedId(null);
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const deleteForever = async (item: VaultItem) => {
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
    const ok = await confirm({
      title: t("trash.emptyTitle"),
      message: t("trash.emptyText", { count: counts.trash }),
      confirmLabel: t("trash.empty"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      const removed = await api.emptyTrash();
      setItems((list) => (list ?? []).filter((i) => i.deletedAt === null));
      setSelectedId(null);
      toast.success(t("trash.emptied", { count: removed }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const toggleFavorite = async (item: VaultItem) => {
    try {
      const saved = await api.saveItem({ ...item, favorite: !item.favorite });
      upsertLocal(saved);
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  // ------------------------------------------------------------- folders

  const createFolder = async (name: string) => {
    try {
      const folder = await api.saveFolder({ id: "", name });
      setFolders((list) => [...list, folder].sort(byName));
      toast.success(t("folder.created", { name: folder.name }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const renameFolder = async (folder: Folder, name: string) => {
    try {
      const saved = await api.saveFolder({ ...folder, name });
      setFolders((list) => list.map((f) => (f.id === saved.id ? saved : f)).sort(byName));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const deleteFolder = async (folder: Folder) => {
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
      if (editing || !selected?.login?.password) return;
      e.preventDefault();
      void copy(selected.login.password, { label: t("field.password"), sensitive: true });
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
      }
    }
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => shortcuts.current(e);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // --------------------------------------------------------------- render

  const renderDetail = () => {
    if (editing) {
      return (
        <ItemEditor
          key={editing.draft.id || "new"}
          draft={editing.draft}
          folders={folders}
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
          onEdit={() => startEdit(selected)}
          onTrash={() => void trash(selected)}
          onRestore={() => void restore(selected)}
          onDeleteForever={() => void deleteForever(selected)}
          onToggleFavorite={() => void toggleFavorite(selected)}
        />
      );
    }
    if (items && counts.all === 0 && filter.kind !== "trash") {
      return (
        <EmptyState
          icon={<FileUp />}
          title={t("detail.firstTitle")}
          hint={t("detail.firstHint")}
          action={
            <button type="button" className="btn btn-secondary" onClick={() => navigate({ kind: "settings", section: "data" })}>
              <FileUp />
              {t("detail.importCta")}
            </button>
          }
        />
      );
    }
    return (
      <div className="detail-placeholder">
        <EmptyState icon={<MousePointerClick />} title={t("detail.noneTitle")} hint={t("detail.noneHint")} />
        <dl className="shortcut-list" aria-label={t("shortcuts.title")}>
          {(
            [
              ["Ctrl F", "shortcuts.search"],
              ["Ctrl N", "shortcuts.new"],
              ["Ctrl ⇧ C", "shortcuts.copyPassword"],
              ["Ctrl B", "shortcuts.copyUsername"],
              ["Ctrl L", "shortcuts.lock"],
            ] as [string, MessageKey][]
          ).map(([keys, label]) => (
            <div key={label}>
              <dt>
                <span className="kbd">{keys}</span>
              </dt>
              <dd>{t(label)}</dd>
            </div>
          ))}
        </dl>
      </div>
    );
  };

  return (
    <div className="shell">
      <Sidebar
        vaultName={vault?.name ?? ""}
        view={view}
        counts={counts}
        folders={folders}
        onNavigate={navigate}
        onLock={() => void lock()}
        onCreateFolder={(name) => void createFolder(name)}
        onRenameFolder={(folder, name) => void renameFolder(folder, name)}
        onDeleteFolder={(folder) => void deleteFolder(folder)}
      />
      {view.kind === "vault" && (
        <div className="vault-view">
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
          />
          <main className="detail-pane" ref={detailRef} aria-label={t("detail.label")}>
            {renderDetail()}
          </main>
        </div>
      )}
      {view.kind === "generator" && <GeneratorPage />}
      {view.kind === "health" && (
        <HealthPage
          items={allItems}
          onOpenItem={(id) => {
            setView({ kind: "vault", filter: { kind: "all" } });
            setQuery("");
            setSelectedId(id);
          }}
        />
      )}
      {view.kind === "settings" && <SettingsPage initialSection={view.section} />}
    </div>
  );
}
