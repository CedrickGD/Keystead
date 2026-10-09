import { forwardRef, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import {
  ArrowUpDown,
  ChevronDown,
  CreditCard,
  IdCard,
  KeyRound,
  Plus,
  Search,
  SearchX,
  Star,
  StickyNote,
  Trash2,
  UserRound,
  X,
  Layers,
} from "lucide-react";
import type { ItemType, VaultItem } from "../../lib/types";
import { itemSubtitle } from "../../lib/utils";
import { useT } from "../../i18n";
import { useCopy } from "../../state/app";
import { Avatar } from "../../components/Avatar";
import { iconHost } from "../../lib/icons";
import { EmptyState, Highlight } from "../../components/EmptyState";
import { Menu, MenuItem } from "../../components/Menu";
import type { Filter, SortKey } from "./model";

export const TYPE_ICONS: Record<ItemType, ReactNode> = {
  login: <KeyRound />,
  card: <CreditCard />,
  identity: <IdCard />,
  note: <StickyNote />,
};

export function NewItemButton({ onNew, compact }: { onNew: (type: ItemType) => void; compact?: boolean }) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  const types: ItemType[] = ["login", "card", "identity", "note"];
  return (
    <div className="popover-anchor">
      <button
        ref={ref}
        type="button"
        className="btn btn-primary new-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        title={`${t("list.new")} (Ctrl+N)`}
      >
        <Plus />
        {!compact && <span>{t("list.new")}</span>}
        <ChevronDown className="new-btn-chevron" />
      </button>
      {open && (
        <Menu label={t("list.newMenu")} anchorRef={ref} align="right" onClose={() => setOpen(false)} width={220}>
          <div className="menu-label">{t("list.newMenu")}</div>
          {types.map((type) => (
            <MenuItem
              key={type}
              icon={TYPE_ICONS[type]}
              onSelect={() => {
                setOpen(false);
                onNew(type);
              }}
            >
              {t(`type.${type}`)}
            </MenuItem>
          ))}
        </Menu>
      )}
    </div>
  );
}

function SortButton({ sort, onSort }: { sort: SortKey; onSort: (sort: SortKey) => void }) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  return (
    <div className="popover-anchor">
      <button
        ref={ref}
        type="button"
        className="btn btn-ghost btn-sm sort-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        title={t("list.sort")}
      >
        <ArrowUpDown />
        <span className="hide-narrow">{sort === "name" ? t("list.sortName") : t("list.sortUpdated")}</span>
      </button>
      {open && (
        <Menu label={t("list.sort")} anchorRef={ref} align="right" onClose={() => setOpen(false)}>
          <div className="menu-label">{t("list.sort")}</div>
          {(["name", "updated"] as SortKey[]).map((key) => (
            <MenuItem
              key={key}
              checked={sort === key}
              onSelect={() => {
                onSort(key);
                setOpen(false);
              }}
            >
              {key === "name" ? t("list.sortName") : t("list.sortUpdated")}
            </MenuItem>
          ))}
        </Menu>
      )}
    </div>
  );
}

export interface ItemListProps {
  title: string;
  filter: Filter;
  items: VaultItem[];
  loading: boolean;
  totalInFilter: number;
  selectedId: string | null;
  onSelect: (id: string) => void;
  onOpen: () => void;
  query: string;
  onQuery: (query: string) => void;
  terms: string[];
  sort: SortKey;
  onSort: (sort: SortKey) => void;
  onNew: (type: ItemType) => void;
  onEmptyTrash: () => void;
}

export const ItemList = forwardRef<HTMLInputElement, ItemListProps>(function ItemList(
  {
    title,
    filter,
    items,
    loading,
    totalInFilter,
    selectedId,
    onSelect,
    onOpen,
    query,
    onQuery,
    terms,
    sort,
    onSort,
    onNew,
    onEmptyTrash,
  },
  searchRef,
) {
  const { t, formatRelative } = useT();
  const copy = useCopy();
  const listRef = useRef<HTMLDivElement>(null);
  const isTrash = filter.kind === "trash";

  // Keep the selected row visible during keyboard navigation.
  useEffect(() => {
    if (!selectedId) return;
    const row = listRef.current?.querySelector<HTMLElement>(`[data-id="${CSS.escape(selectedId)}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }, [selectedId]);

  const move = (delta: number, focusRow: boolean) => {
    if (items.length === 0) return;
    const idx = items.findIndex((i) => i.id === selectedId);
    const nextIdx = idx < 0 ? (delta > 0 ? 0 : items.length - 1) : Math.min(items.length - 1, Math.max(0, idx + delta));
    const next = items[nextIdx];
    if (!next) return;
    onSelect(next.id);
    if (focusRow) {
      requestAnimationFrame(() =>
        listRef.current?.querySelector<HTMLElement>(`[data-id="${CSS.escape(next.id)}"]`)?.focus(),
      );
    }
  };

  const onListKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      move(e.key === "ArrowDown" ? 1 : -1, true);
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      move(e.key === "Home" ? -items.length : items.length, true);
    } else if (e.key === "Enter") {
      e.preventDefault();
      onOpen();
    }
  };

  const onSearchKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      move(e.key === "ArrowDown" ? 1 : -1, false);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!selectedId && items[0]) onSelect(items[0].id);
      onOpen();
    } else if (e.key === "Escape" && query) {
      e.preventDefault();
      e.stopPropagation();
      onQuery("");
    }
  };

  let body: ReactNode;
  if (loading) {
    body = (
      <div className="list-skeleton" aria-hidden>
        {Array.from({ length: 7 }, (_, i) => (
          <div key={i} className="skeleton-row">
            <div className="skeleton-avatar" />
            <div className="skeleton-lines">
              <div style={{ width: `${55 + ((i * 17) % 35)}%` }} />
              <div style={{ width: `${35 + ((i * 23) % 30)}%` }} />
            </div>
          </div>
        ))}
      </div>
    );
  } else if (items.length === 0) {
    body = terms.length ? (
      <EmptyState icon={<SearchX />} title={t("list.noResults")} hint={t("list.noResultsHint", { query: query.trim() })} />
    ) : isTrash ? (
      <EmptyState icon={<Trash2 />} title={t("list.trashEmpty")} hint={t("list.trashEmptyHint")} />
    ) : filter.kind === "favorites" ? (
      <EmptyState icon={<Star />} title={t("list.noFavorites")} hint={t("list.noFavoritesHint")} />
    ) : filter.kind === "all" ? (
      // The detail pane shows the "get started" actions for an empty vault.
      <EmptyState icon={<Layers />} title={t("list.empty")} />
    ) : (
      <EmptyState
        icon={<Layers />}
        title={t("list.empty")}
        hint={t("list.emptyHint")}
        action={
          <button type="button" className="btn btn-secondary btn-sm" onClick={() => onNew(filter.kind === "type" ? filter.type : "login")}>
            <Plus />
            {t("list.addFirst")}
          </button>
        }
      />
    );
  } else {
    body = (
      <div className="list-rows" role="listbox" aria-label={title} onKeyDown={onListKey} ref={listRef}>
        {items.map((item) => {
          const subtitle = itemSubtitle(item);
          const selected = item.id === selectedId;
          return (
            <div
              key={item.id}
              data-id={item.id}
              role="option"
              aria-selected={selected}
              tabIndex={selected || (!selectedId && item === items[0]) ? 0 : -1}
              className={`row ${selected ? "selected" : ""}`}
              onClick={() => onSelect(item.id)}
              onDoubleClick={onOpen}
            >
              <Avatar name={item.name} type={item.type} site={iconHost(item)} />
              <div className="row-main">
                <div className="row-name truncate">
                  <Highlight text={item.name} terms={terms} />
                </div>
                <div className="row-sub truncate">
                  {isTrash && item.deletedAt ? (
                    t("list.deletedAgo", { when: formatRelative(item.deletedAt) })
                  ) : subtitle ? (
                    <Highlight text={subtitle} terms={terms} />
                  ) : (
                    <span className="subtle">{t(`type.${item.type}`)}</span>
                  )}
                </div>
              </div>
              {item.favorite && !isTrash && <Star className="row-star" aria-label={t("item.favorite")} />}
              {!isTrash && item.login && (item.login.username || item.login.password) && (
                // Mouse shortcut for the most common task; keyboard users have Ctrl+B / Ctrl+Shift+C.
                <div className="row-actions">
                  {item.login.username && (
                    <button
                      type="button"
                      className="icon-btn sm"
                      tabIndex={-1}
                      title={t("common.copyNamed", { what: t("field.username") })}
                      aria-label={t("common.copyNamed", { what: t("field.username") })}
                      onClick={(e) => {
                        e.stopPropagation();
                        void copy(item.login?.username ?? "", { label: t("field.username"), sensitive: false });
                      }}
                    >
                      <UserRound />
                    </button>
                  )}
                  {item.login.password && (
                    <button
                      type="button"
                      className="icon-btn sm"
                      tabIndex={-1}
                      title={t("common.copyNamed", { what: t("field.password") })}
                      aria-label={t("common.copyNamed", { what: t("field.password") })}
                      onClick={(e) => {
                        e.stopPropagation();
                        void copy(item.login?.password ?? "", { label: t("field.password"), sensitive: true });
                      }}
                    >
                      <KeyRound />
                    </button>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    );
  }

  return (
    <section className="list-pane" aria-label={title}>
      <div className="list-header">
        <div className="list-header-row">
          <div className="search">
            <Search className="search-icon" aria-hidden />
            <input
              ref={searchRef}
              className="input search-input"
              type="search"
              value={query}
              placeholder={t("list.searchPlaceholder")}
              aria-label={t("list.search")}
              onChange={(e) => onQuery(e.target.value)}
              onKeyDown={onSearchKey}
              spellCheck={false}
              autoComplete="off"
            />
            {query ? (
              <button type="button" className="icon-btn sm search-clear" onClick={() => onQuery("")} aria-label={t("list.clearSearch")}>
                <X />
              </button>
            ) : (
              <span className="kbd search-kbd" aria-hidden>
                Ctrl F
              </span>
            )}
          </div>
          {!isTrash && <NewItemButton onNew={onNew} />}
        </div>
        <div className="list-toolbar">
          <div className="list-title truncate">
            <span>{title}</span>
            <span className="list-count">
              {terms.length ? t("list.countFiltered", { shown: items.length, total: totalInFilter }) : items.length}
            </span>
          </div>
          {isTrash ? (
            <button
              type="button"
              className="btn btn-danger-ghost btn-sm"
              onClick={onEmptyTrash}
              disabled={totalInFilter === 0}
              title={t("trash.empty")}
            >
              <Trash2 />
              {t("trash.emptyShort")}
            </button>
          ) : (
            <SortButton sort={sort} onSort={onSort} />
          )}
        </div>
      </div>
      <div className="list-scroll">{body}</div>
    </section>
  );
});
