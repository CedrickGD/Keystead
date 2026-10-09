import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Check,
  CreditCard,
  Folder as FolderIcon,
  FolderPlus,
  IdCard,
  KeyRound,
  Layers,
  Lock,
  LockOpen,
  Pencil,
  Settings as SettingsIcon,
  ShieldCheck,
  Star,
  StickyNote,
  Trash2,
  WandSparkles,
  X,
} from "lucide-react";
import type { Folder, ItemType } from "../../lib/types";
import { useT } from "../../i18n";
import { Logo } from "../../components/Logo";
import { useEscapeLayer } from "../../lib/layers";
import { sameFilter, type Counts, type Filter, type View } from "./model";

function NavItem({
  icon,
  label,
  count,
  active,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  count?: number;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button type="button" className={`nav-item ${active ? "active" : ""}`} onClick={onClick} aria-current={active ? "page" : undefined}>
      {icon}
      <span className="nav-label truncate">{label}</span>
      {count !== undefined && count > 0 && <span className="nav-count">{count}</span>}
    </button>
  );
}

function FolderNameInput({
  initial,
  placeholder,
  onCommit,
  onCancel,
}: {
  initial: string;
  placeholder: string;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const { t } = useT();
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  // Blur, Enter and the buttons can all finish editing – only act once.
  const done = useRef(false);
  const cancel = () => {
    if (done.current) return;
    done.current = true;
    onCancel();
  };
  useEscapeLayer(cancel);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const commit = () => {
    if (done.current) return;
    const name = value.trim();
    done.current = true;
    if (name) onCommit(name);
    else onCancel();
  };
  return (
    <div className="nav-edit">
      <FolderIcon className="nav-edit-icon" />
      <input
        ref={ref}
        className="input nav-edit-input"
        value={value}
        placeholder={placeholder}
        maxLength={60}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
        onBlur={commit}
        aria-label={placeholder}
      />
      <button type="button" className="icon-btn sm" onMouseDown={(e) => e.preventDefault()} onClick={commit} aria-label={t("common.save")}>
        <Check />
      </button>
      <button type="button" className="icon-btn sm" onMouseDown={(e) => e.preventDefault()} onClick={cancel} aria-label={t("common.cancel")}>
        <X />
      </button>
    </div>
  );
}

export function Sidebar({
  vaultName,
  view,
  counts,
  folders,
  onNavigate,
  onLock,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
}: {
  vaultName: string;
  view: View;
  counts: Counts;
  folders: Folder[];
  onNavigate: (view: View) => void;
  onLock: () => void;
  onCreateFolder: (name: string) => void;
  onRenameFolder: (folder: Folder, name: string) => void;
  onDeleteFolder: (folder: Folder) => void;
}) {
  const { t } = useT();
  const [editing, setEditing] = useState<string | "new" | null>(null);

  const isFilter = (filter: Filter) => view.kind === "vault" && sameFilter(view.filter, filter);
  const go = (filter: Filter) => onNavigate({ kind: "vault", filter });

  const types: { type: ItemType; label: string; icon: ReactNode }[] = [
    { type: "login", label: t("type.login.plural"), icon: <KeyRound /> },
    { type: "card", label: t("type.card.plural"), icon: <CreditCard /> },
    { type: "identity", label: t("type.identity.plural"), icon: <IdCard /> },
    { type: "note", label: t("type.note.plural"), icon: <StickyNote /> },
  ];

  return (
    <aside className="sidebar" aria-label={t("sidebar.label")}>
      <div className="sidebar-header">
        <Logo size={28} />
        <div className="sidebar-brand">
          <div className="sidebar-brand-name">Keystead</div>
          <div className="sidebar-vault-name truncate" title={t("sidebar.vaultTitle", { name: vaultName })}>
            <LockOpen aria-hidden />
            <span className="truncate">{vaultName}</span>
          </div>
        </div>
        <button
          type="button"
          className="icon-btn"
          onClick={onLock}
          title={`${t("sidebar.lock")} (Ctrl+L)`}
          aria-label={t("sidebar.lock")}
        >
          <Lock />
        </button>
      </div>

      <nav className="sidebar-scroll">
        <div className="nav-group">
          <NavItem icon={<Layers />} label={t("nav.all")} count={counts.all} active={isFilter({ kind: "all" })} onClick={() => go({ kind: "all" })} />
          <NavItem
            icon={<Star />}
            label={t("nav.favorites")}
            count={counts.favorites}
            active={isFilter({ kind: "favorites" })}
            onClick={() => go({ kind: "favorites" })}
          />
        </div>

        <div className="nav-section-title">{t("nav.types")}</div>
        <div className="nav-group">
          {types.map(({ type, label, icon }) => (
            <NavItem
              key={type}
              icon={icon}
              label={label}
              count={counts[type]}
              active={isFilter({ kind: "type", type })}
              onClick={() => go({ kind: "type", type })}
            />
          ))}
        </div>

        <div className="nav-section-title">
          <span>{t("nav.folders")}</span>
          <button
            type="button"
            className="icon-btn sm nav-section-action"
            onClick={() => setEditing("new")}
            title={t("folder.new")}
            aria-label={t("folder.new")}
          >
            <FolderPlus />
          </button>
        </div>
        <div className="nav-group">
          {folders.map((folder) =>
            editing === folder.id ? (
              <FolderNameInput
                key={folder.id}
                initial={folder.name}
                placeholder={t("folder.name")}
                onCommit={(name) => {
                  setEditing(null);
                  if (name !== folder.name) onRenameFolder(folder, name);
                }}
                onCancel={() => setEditing(null)}
              />
            ) : (
              <div key={folder.id} className="nav-row">
                <NavItem
                  icon={<FolderIcon />}
                  label={folder.name}
                  count={counts.folders[folder.id] ?? 0}
                  active={isFilter({ kind: "folder", id: folder.id })}
                  onClick={() => go({ kind: "folder", id: folder.id })}
                />
                <div className="nav-row-actions">
                  <button
                    type="button"
                    className="icon-btn sm"
                    onClick={() => setEditing(folder.id)}
                    title={t("folder.rename")}
                    aria-label={t("folder.renameNamed", { name: folder.name })}
                  >
                    <Pencil />
                  </button>
                  <button
                    type="button"
                    className="icon-btn sm danger"
                    onClick={() => onDeleteFolder(folder)}
                    title={t("folder.delete")}
                    aria-label={t("folder.deleteNamed", { name: folder.name })}
                  >
                    <Trash2 />
                  </button>
                </div>
              </div>
            ),
          )}
          {editing === "new" && (
            <FolderNameInput
              initial=""
              placeholder={t("folder.name")}
              onCommit={(name) => {
                setEditing(null);
                onCreateFolder(name);
              }}
              onCancel={() => setEditing(null)}
            />
          )}
          {folders.length === 0 && editing !== "new" && (
            <button type="button" className="nav-hint" onClick={() => setEditing("new")}>
              {t("folder.emptyHint")}
            </button>
          )}
        </div>

        <div className="nav-group nav-group-spaced">
          <NavItem icon={<Trash2 />} label={t("nav.trash")} count={counts.trash} active={isFilter({ kind: "trash" })} onClick={() => go({ kind: "trash" })} />
        </div>

        <div className="nav-section-title">{t("nav.tools")}</div>
        <div className="nav-group">
          <NavItem
            icon={<WandSparkles />}
            label={t("nav.generator")}
            active={view.kind === "generator"}
            onClick={() => onNavigate({ kind: "generator" })}
          />
          <NavItem
            icon={<ShieldCheck />}
            label={t("nav.health")}
            active={view.kind === "health"}
            onClick={() => onNavigate({ kind: "health" })}
          />
        </div>
      </nav>

      <div className="sidebar-footer">
        <NavItem
          icon={<SettingsIcon />}
          label={t("nav.settings")}
          active={view.kind === "settings"}
          onClick={() => onNavigate({ kind: "settings" })}
        />
      </div>
    </aside>
  );
}
