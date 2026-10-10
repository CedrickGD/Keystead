import { useRef, useState } from "react";
import {
  Check,
  ChevronRight,
  Folder as FolderIcon,
  History,
  MoreHorizontal,
  Pencil,
  RotateCcw,
  Star,
  Trash2,
  X,
} from "lucide-react";
import type { Folder, ItemListEntry } from "../../lib/types";
import { normalizeUrl } from "../../lib/api";
import { formatCardNumber, identityFullName } from "../../lib/utils";
import { useT } from "../../i18n";
import { CopyButton } from "../../components/CopyButton";
import { Avatar } from "../../components/Avatar";
import { iconHost } from "../../lib/icons";
import { Menu, MenuItem } from "../../components/Menu";
import { TotpValue, useItemTotp } from "../../components/Totp";
import { FieldRow, SecretFieldRow, Section } from "./FieldRow";
import { TYPE_ICONS } from "./ItemList";

// The item list holds no secrets (`ItemListEntry`): every secret row fetches
// its value on demand (`reveal_secret`) and copies in the backend
// (`copy_secret_field`); see `SecretFieldRow`.

function TotpRow({ item }: { item: ItemListEntry }) {
  const { t } = useT();
  const totp = useItemTotp(item.id, item.updatedAt);
  return (
    <div className="fieldrow">
      <div className="fieldrow-main">
        <div className="fieldrow-label">{t("field.totp")}</div>
        <div className="fieldrow-value">
          <TotpValue totp={totp} />
        </div>
      </div>
      <div className="fieldrow-actions">
        <CopyButton secret={{ itemId: item.id, field: "totpCode" }} label={t("field.totpShort")} disabled={!totp.code} />
      </div>
    </div>
  );
}

function PasswordHistory({ item }: { item: ItemListEntry }) {
  const { t, formatDateTime } = useT();
  const [open, setOpen] = useState(false);
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
          {item.passwordHistory.map((entry, idx) => (
            // Only the date is known until an entry is revealed.
            <SecretFieldRow
              key={idx}
              itemId={item.id}
              field={{ history: idx }}
              revision={item.updatedAt}
              label={t("item.replacedAt", { date: formatDateTime(entry.replacedAt) })}
              copyLabel={t("field.password")}
              password
            />
          ))}
        </div>
      )}
    </section>
  );
}

function CustomFields({ item }: { item: ItemListEntry }) {
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
        ) : field.kind === "hidden" ? (
          <SecretFieldRow
            key={idx}
            itemId={item.id}
            field={{ custom: idx }}
            revision={item.updatedAt}
            label={field.name || t("item.unnamedField")}
            copyLabel={field.name || t("item.value")}
            mono
          />
        ) : (
          <FieldRow
            key={idx}
            label={field.name || t("item.unnamedField")}
            value={field.value}
            copyLabel={field.name || t("item.value")}
          />
        ),
      )}
    </Section>
  );
}

function Notes({ item }: { item: ItemListEntry }) {
  const { t } = useT();
  if (!item.notes.trim()) return null;
  return (
    <Section title={t("field.notes")}>
      {/* Notes of any item type often hold backup codes or security answers: copy as a secret. */}
      <FieldRow value={item.notes} multiline copyLabel={t("field.notes")} sensitive />
    </Section>
  );
}

function LoginBody({ item }: { item: ItemListEntry }) {
  const { t } = useT();
  const login = item.login;
  if (!login) return null;
  const uris = login.uris.filter((u) => u.uri.trim());
  const hasCredentials = login.username || login.hasPassword || login.hasTotp;
  return (
    <>
      {hasCredentials && (
        <Section title={t("item.credentials")}>
          {login.username && <FieldRow label={t("field.username")} value={login.username} copyLabel={t("field.username")} />}
          {login.hasPassword && (
            <SecretFieldRow itemId={item.id} field="password" revision={item.updatedAt} label={t("field.password")} password />
          )}
          {login.hasTotp && <TotpRow item={item} />}
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

function CardBody({ item }: { item: ItemListEntry }) {
  const { t } = useT();
  const card = item.card;
  if (!card) return null;
  const expiry = card.expMonth || card.expYear ? `${card.expMonth || "––"} / ${card.expYear || "––––"}` : "";
  return (
    <Section title={t("item.cardDetails")}>
      {card.cardholderName && <FieldRow label={t("field.cardholder")} value={card.cardholderName} copyLabel={t("field.cardholder")} />}
      {card.hasNumber && (
        <SecretFieldRow
          itemId={item.id}
          field="cardNumber"
          revision={item.updatedAt}
          label={t("field.cardNumber")}
          format={formatCardNumber}
          mono
        />
      )}
      {card.brand && <FieldRow label={t("field.brand")} value={card.brand} />}
      {expiry && <FieldRow label={t("field.expiry")} value={expiry} copyLabel={t("field.expiry")} />}
      {card.hasCode && (
        <SecretFieldRow itemId={item.id} field="cardCode" revision={item.updatedAt} label={t("field.securityCode")} mono />
      )}
    </Section>
  );
}

function IdentityBody({ item }: { item: ItemListEntry }) {
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
  item: ItemListEntry;
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
