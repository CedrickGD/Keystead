import { useRef, useState } from "react";
import {
  Check,
  ChevronRight,
  Eye,
  EyeOff,
  Folder as FolderIcon,
  History,
  MoreHorizontal,
  Pencil,
  RotateCcw,
  Star,
  Trash2,
  X,
} from "lucide-react";
import type { Folder, VaultItem } from "../../lib/types";
import { normalizeUrl } from "../../lib/api";
import { formatCardNumber, identityFullName } from "../../lib/utils";
import { useT } from "../../i18n";
import { CopyButton } from "../../components/CopyButton";
import { Avatar } from "../../components/Avatar";
import { iconHost } from "../../lib/icons";
import { Menu, MenuItem } from "../../components/Menu";
import { TotpValue, useTotp } from "../../components/Totp";
import { PasswordText } from "../../components/PasswordText";
import { FieldRow, Section } from "./FieldRow";
import { TYPE_ICONS } from "./ItemList";

function TotpRow({ seed }: { seed: string }) {
  const { t } = useT();
  const totp = useTotp(seed);
  return (
    <div className="fieldrow">
      <div className="fieldrow-main">
        <div className="fieldrow-label">{t("field.totp")}</div>
        <div className="fieldrow-value">
          <TotpValue totp={totp} />
        </div>
      </div>
      <div className="fieldrow-actions">
        <CopyButton value={totp.code?.code ?? ""} label={t("field.totpShort")} sensitive disabled={!totp.code} />
      </div>
    </div>
  );
}

function PasswordHistory({ item }: { item: VaultItem }) {
  const { t, formatDateTime } = useT();
  const [open, setOpen] = useState(false);
  const [revealed, setRevealed] = useState<Set<number>>(new Set());
  if (item.passwordHistory.length === 0) return null;
  return (
    <section className="detail-section">
      <button type="button" className="collapse-toggle" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <ChevronRight className="collapse-chevron" />
        <History />
        <span>{t("item.passwordHistory")}</span>
        <span className="chip">{item.passwordHistory.length}</span>
      </button>
      {open && (
        <div className="card field-card">
          {item.passwordHistory.map((entry, idx) => {
            const shown = revealed.has(idx);
            return (
              <div className="fieldrow" key={idx}>
                <div className="fieldrow-main">
                  <div className="fieldrow-label">{t("item.replacedAt", { date: formatDateTime(entry.replacedAt) })}</div>
                  <div className="fieldrow-value selectable">
                    {shown ? <PasswordText value={entry.password} /> : <span className="mask">••••••••••••</span>}
                  </div>
                </div>
                <div className="fieldrow-actions">
                  <button
                    type="button"
                    className="icon-btn"
                    aria-pressed={shown}
                    title={shown ? t("common.hide") : t("common.show")}
                    aria-label={shown ? t("common.hide") : t("common.show")}
                    onClick={() =>
                      setRevealed((set) => {
                        const next = new Set(set);
                        if (next.has(idx)) next.delete(idx);
                        else next.add(idx);
                        return next;
                      })
                    }
                  >
                    {shown ? <EyeOff /> : <Eye />}
                  </button>
                  <CopyButton value={entry.password} label={t("field.password")} sensitive />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function CustomFields({ item }: { item: VaultItem }) {
  const { t } = useT();
  if (item.fields.length === 0) return null;
  return (
    <Section title={t("item.customFields")}>
      {item.fields.map((field, idx) =>
        field.kind === "boolean" ? (
          <FieldRow
            key={idx}
            label={field.name || t("item.unnamedField")}
            value={field.value}
            display={
              <span className="bool-value">
                {field.value === "true" ? <Check className="bool-yes" /> : <X className="bool-no" />}
                {field.value === "true" ? t("common.yes") : t("common.no")}
              </span>
            }
          />
        ) : (
          <FieldRow
            key={idx}
            label={field.name || t("item.unnamedField")}
            value={field.value}
            secret={field.kind === "hidden"}
            mono={field.kind === "hidden"}
            copyLabel={field.name || t("item.value")}
            sensitive={field.kind === "hidden"}
          />
        ),
      )}
    </Section>
  );
}

function Notes({ item }: { item: VaultItem }) {
  const { t } = useT();
  if (!item.notes.trim()) return null;
  return (
    <Section title={t("field.notes")}>
      {/* Notes of any item type often hold backup codes or security answers: copy as a secret. */}
      <FieldRow value={item.notes} multiline copyLabel={t("field.notes")} sensitive />
    </Section>
  );
}

function LoginBody({ item }: { item: VaultItem }) {
  const { t } = useT();
  const login = item.login;
  if (!login) return null;
  const uris = login.uris.filter((u) => u.uri.trim());
  const hasCredentials = login.username || login.password || login.totp;
  return (
    <>
      {hasCredentials && (
        <Section title={t("item.credentials")}>
          {login.username && <FieldRow label={t("field.username")} value={login.username} copyLabel={t("field.username")} />}
          {login.password && (
            <FieldRow label={t("field.password")} value={login.password} secret password copyLabel={t("field.password")} sensitive />
          )}
          {login.totp.trim() && <TotpRow seed={login.totp} />}
        </Section>
      )}
      {uris.length > 0 && (
        <Section title={uris.length === 1 ? t("field.website") : t("field.websites")}>
          {uris.map((u, idx) => {
            const href = normalizeUrl(u.uri);
            return (
              <FieldRow
                key={idx}
                label={uris.length === 1 ? undefined : t("field.websiteN", { n: idx + 1 })}
                value={u.uri}
                href={href ?? undefined}
                copyLabel={t("field.url")}
              />
            );
          })}
        </Section>
      )}
    </>
  );
}

function CardBody({ item }: { item: VaultItem }) {
  const { t } = useT();
  const card = item.card;
  if (!card) return null;
  const expiry = card.expMonth || card.expYear ? `${card.expMonth || "––"} / ${card.expYear || "––––"}` : "";
  return (
    <Section title={t("item.cardDetails")}>
      {card.cardholderName && <FieldRow label={t("field.cardholder")} value={card.cardholderName} copyLabel={t("field.cardholder")} />}
      {card.number && (
        <FieldRow
          label={t("field.cardNumber")}
          value={card.number.replace(/\s+/g, "")}
          display={formatCardNumber(card.number)}
          mono
          secret
          copyLabel={t("field.cardNumber")}
          sensitive
        />
      )}
      {card.brand && <FieldRow label={t("field.brand")} value={card.brand} />}
      {expiry && <FieldRow label={t("field.expiry")} value={expiry} copyLabel={t("field.expiry")} />}
      {card.code && <FieldRow label={t("field.securityCode")} value={card.code} mono secret copyLabel={t("field.securityCode")} sensitive />}
    </Section>
  );
}

function IdentityBody({ item }: { item: VaultItem }) {
  const { t } = useT();
  const id = item.identity;
  if (!id) return null;
  const fullName = [id.title, identityFullName(id)].filter(Boolean).join(" ");
  const cityLine = [id.postalCode, id.city].filter(Boolean).join(" ");
  const address = [id.address1, id.address2, cityLine, id.state, id.country].filter((s) => s.trim()).join("\n");
  const hasPersonal = fullName || id.username || id.company;
  const hasContact = id.email || id.phone;
  return (
    <>
      {hasPersonal && (
        <Section title={t("item.personal")}>
          {fullName && <FieldRow label={t("field.name")} value={fullName} copyLabel={t("field.name")} />}
          {id.username && <FieldRow label={t("field.username")} value={id.username} copyLabel={t("field.username")} />}
          {id.company && <FieldRow label={t("field.company")} value={id.company} copyLabel={t("field.company")} />}
        </Section>
      )}
      {hasContact && (
        <Section title={t("item.contact")}>
          {id.email && <FieldRow label={t("field.email")} value={id.email} copyLabel={t("field.email")} />}
          {id.phone && <FieldRow label={t("field.phone")} value={id.phone} copyLabel={t("field.phone")} />}
        </Section>
      )}
      {address && (
        <Section title={t("item.address")}>
          <FieldRow value={address} multiline copyLabel={t("field.address")} />
        </Section>
      )}
    </>
  );
}

export interface ItemViewProps {
  item: VaultItem;
  folders: Folder[];
  onEdit: () => void;
  onTrash: () => void;
  onRestore: () => void;
  onDeleteForever: () => void;
  onToggleFavorite: () => void;
}

export function ItemView({ item, folders, onEdit, onTrash, onRestore, onDeleteForever, onToggleFavorite }: ItemViewProps) {
  const { t, formatDateTime, formatRelative } = useT();
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLButtonElement>(null);
  const folder = folders.find((f) => f.id === item.folderId);
  const trashed = item.deletedAt !== null;

  return (
    <div className="detail" tabIndex={-1} data-detail-root>
      <header className="detail-header">
        <Avatar name={item.name} type={item.type} size="lg" site={iconHost(item)} />
        <div className="detail-title">
          <h1 className="selectable" title={item.name}>
            {item.name}
          </h1>
          <div className="detail-chips">
            <span className="chip">
              {TYPE_ICONS[item.type]}
              {t(`type.${item.type}`)}
            </span>
            {folder && (
              <span className="chip">
                <FolderIcon />
                {folder.name}
              </span>
            )}
          </div>
        </div>
        {!trashed && (
          <div className="detail-actions">
            <button
              type="button"
              className={`icon-btn ${item.favorite ? "on" : ""}`}
              onClick={onToggleFavorite}
              aria-pressed={item.favorite}
              title={item.favorite ? t("item.unfavorite") : t("item.makeFavorite")}
              aria-label={item.favorite ? t("item.unfavorite") : t("item.makeFavorite")}
            >
              <Star fill={item.favorite ? "currentColor" : "none"} />
            </button>
            <button type="button" className="btn btn-secondary edit-btn" onClick={onEdit} data-detail-primary title={t("common.edit")}>
              <Pencil />
              <span className="btn-label">{t("common.edit")}</span>
            </button>
            <div className="popover-anchor">
              <button
                ref={menuRef}
                type="button"
                className="icon-btn"
                aria-haspopup="menu"
                aria-expanded={menuOpen}
                onClick={() => setMenuOpen((o) => !o)}
                title={t("common.more")}
                aria-label={t("common.more")}
              >
                <MoreHorizontal />
              </button>
              {menuOpen && (
                <Menu label={t("common.more")} anchorRef={menuRef} align="right" onClose={() => setMenuOpen(false)}>
                  <MenuItem
                    icon={<Trash2 />}
                    danger
                    onSelect={() => {
                      setMenuOpen(false);
                      onTrash();
                    }}
                  >
                    {t("item.moveToTrash")}
                  </MenuItem>
                </Menu>
              )}
            </div>
          </div>
        )}
      </header>

      <div className="detail-body">
        {trashed && (
          <div className="trash-banner">
            <Trash2 />
            <div className="trash-banner-text">
              <strong>{t("trash.itemInTrash")}</strong>
              <span>{t("trash.deletedAt", { when: formatRelative(item.deletedAt ?? 0) })}</span>
            </div>
            <div className="trash-banner-actions">
              <button type="button" className="btn btn-secondary btn-sm" onClick={onRestore} data-detail-primary>
                <RotateCcw />
                {t("trash.restore")}
              </button>
              <button type="button" className="btn btn-danger-ghost btn-sm" onClick={onDeleteForever}>
                {t("trash.deleteForever")}
              </button>
            </div>
          </div>
        )}

        {item.type === "login" && <LoginBody item={item} />}
        {item.type === "card" && <CardBody item={item} />}
        {item.type === "identity" && <IdentityBody item={item} />}
        <Notes item={item} />
        <CustomFields item={item} />
        {item.type === "login" && <PasswordHistory item={item} />}

        <dl className="detail-meta">
          <div>
            <dt>{t("item.updated")}</dt>
            <dd>{formatDateTime(item.updatedAt)}</dd>
          </div>
          <div>
            <dt>{t("item.created")}</dt>
            <dd>{formatDateTime(item.createdAt)}</dd>
          </div>
          {item.login?.passwordRevisedAt && (
            <div>
              <dt>{t("item.passwordChanged")}</dt>
              <dd>{formatDateTime(item.login.passwordRevisedAt)}</dd>
            </div>
          )}
        </dl>
      </div>
    </div>
  );
}
