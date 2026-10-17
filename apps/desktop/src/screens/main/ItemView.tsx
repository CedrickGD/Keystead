import { useRef, useState } from "react";
import {
  ArrowLeft,
  Check,
  ChevronRight,
  Download,
  Eye,
  EyeOff,
  FileText,
  Folder as FolderIcon,
  History,
  Image as ImageIcon,
  MoreHorizontal,
  Paperclip,
  Pencil,
  RotateCcw,
  ShieldAlert,
  Star,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import { ATTACHMENTS_PER_ITEM_MAX, type AttachmentListEntry, type Folder, type ItemListEntry } from "../../lib/types";
import { normalizeUrl } from "../../lib/api";
import { formatBytes, formatCardNumber, identityFullName } from "../../lib/utils";
import { useT } from "../../i18n";
import { CopyButton } from "../../components/CopyButton";
import { Avatar } from "../../components/Avatar";
import { Button } from "../../components/Controls";
import { iconHost } from "../../lib/icons";
import { Menu, MenuItem } from "../../components/Menu";
import { TotpValue, useItemTotp } from "../../components/Totp";
import { useRevealedAttachment } from "../../components/useRevealedAttachment";
import { useBreaches } from "../../state/breaches";
import { FieldRow, SecretFieldRow, Section } from "./FieldRow";
import { TYPE_ICONS } from "./ItemList";

// The item list holds no secrets (`ItemListEntry`): every secret row fetches
// its value on demand (`reveal_secret`) and copies in the backend
// (`copy_secret_field`); see `SecretFieldRow`. Attachments are listed by
// name, size and date only; their bytes stay in the backend too.

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

/** Only these get the in-app preview (the backend checks the magic bytes, not the name). */
const IMAGE_NAME = /\.(png|jpe?g|gif|webp)$/i;

function AttachmentRow({
  item,
  attachment,
  readOnly,
  canSave,
  onSave,
  onRemove,
}: {
  item: ItemListEntry;
  attachment: AttachmentListEntry;
  readOnly: boolean;
  /** "Speichern unter …" exists (a native save dialog: not on the phone). */
  canSave: boolean;
  onSave: () => void;
  onRemove: () => void;
}) {
  const { t, locale, formatDate } = useT();
  const image = IMAGE_NAME.test(attachment.name);
  const preview = useRevealedAttachment(item.id, attachment.id, item.updatedAt);
  const shown = preview.value !== null;
  const previewLabel = shown ? t("attachments.previewHide") : t("attachments.preview");
  return (
    <>
      <div className="attachment-row">
        <div className="attachment-icon" aria-hidden>
          {image ? <ImageIcon /> : <FileText />}
        </div>
        <div className="attachment-main">
          <div className="attachment-name selectable" title={attachment.name}>
            {attachment.name}
          </div>
          <div className="attachment-meta">
            {formatBytes(attachment.size, locale)} · {formatDate(attachment.createdAt)}
          </div>
        </div>
        <div className="fieldrow-actions">
          {image && (
            <button
              type="button"
              className="icon-btn"
              onClick={preview.toggle}
              aria-busy={preview.loading}
              aria-pressed={shown}
              title={previewLabel}
              aria-label={`${previewLabel}: ${attachment.name}`}
            >
              {shown ? <EyeOff /> : <Eye />}
            </button>
          )}
          {canSave && (
            <button
              type="button"
              className="icon-btn"
              onClick={onSave}
              title={t("attachments.save")}
              aria-label={`${t("attachments.save")}: ${attachment.name}`}
            >
              <Download />
            </button>
          )}
          {!readOnly && (
            <button
              type="button"
              className="icon-btn danger"
              onClick={onRemove}
              title={t("attachments.remove")}
              aria-label={`${t("attachments.remove")}: ${attachment.name}`}
            >
              <Trash2 />
            </button>
          )}
        </div>
      </div>
      {(shown || preview.loading) && (
        <div className={`attachment-preview ${shown ? "" : "loading"}`}>
          {preview.value !== null ? (
            // Saving goes through the backend ("Speichern unter …"), not the webview's image menu.
            <img
              src={preview.value}
              alt={t("attachments.previewAlt", { name: attachment.name })}
              draggable={false}
              onContextMenu={(e) => e.preventDefault()}
            />
          ) : (
            <span className="spinner" aria-hidden />
          )}
        </div>
      )}
    </>
  );
}

/**
 * The item's attachments: preview (images), "Speichern unter …" and
 * "Entfernen" per file, "Datei anhängen …" below. A trashed item shows its
 * files read-only (preview and save still work, nothing changes); so does a
 * read-only vault (the phone), which has no "Speichern unter …" either.
 */
function Attachments({
  item,
  attaching,
  vaultReadOnly,
  canSave,
  onAdd,
  onSave,
  onRemove,
}: {
  item: ItemListEntry;
  attaching: boolean;
  vaultReadOnly: boolean;
  canSave: boolean;
  onAdd: () => void;
  onSave: (attachment: AttachmentListEntry) => void;
  onRemove: (attachment: AttachmentListEntry) => void;
}) {
  const { t } = useT();
  const trashed = item.deletedAt !== null;
  const readOnly = trashed || vaultReadOnly;
  const list = item.attachments;
  if (readOnly && list.length === 0) return null;
  const full = list.length >= ATTACHMENTS_PER_ITEM_MAX;
  return (
    <section className="detail-section">
      <div className="section-label">
        <span>{t("attachments.title")}</span>
        {list.length > 0 && <span className="chip">{list.length}</span>}
      </div>
      <div className="card field-card">
        {list.map((attachment) => (
          <AttachmentRow
            key={attachment.id}
            item={item}
            attachment={attachment}
            readOnly={readOnly}
            canSave={canSave}
            onSave={() => onSave(attachment)}
            onRemove={() => onRemove(attachment)}
          />
        ))}
        {list.length === 0 && <div className="attachment-empty">{t("attachments.empty")}</div>}
        {!readOnly && (
          <div className="attachment-add">
            <Button variant="secondary" size="sm" icon={<Paperclip />} loading={attaching} disabled={full} onClick={onAdd}>
              {t("attachments.add")}
            </Button>
            <span className="attachment-limits">{full ? t("attachments.full") : t("attachments.limits")}</span>
          </div>
        )}
      </div>
    </section>
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

/**
 * The last Have I Been Pwned check of the security report found this login's
 * password (see `state/breaches.tsx`; gone once the item changes).
 * "Passwort ändern" opens the editor.
 */
function LeakWarning({ item, onEdit }: { item: ItemListEntry; onEdit?: () => void }) {
  const { t, tp, locale } = useT();
  const { breachCount } = useBreaches();
  const count = breachCount(item);
  if (count === null) return null;
  return (
    <div className="callout callout-danger pwned-callout" role="note">
      <ShieldAlert />
      <div className="pwned-callout-text">
        <strong>{t("pwned.itemTitle")}</strong>
        <span>{tp("pwned.itemText", count, { n: new Intl.NumberFormat(locale).format(count) })}</span>
      </div>
      {onEdit && (
        <Button variant="danger" size="sm" onClick={onEdit}>
          {t("pwned.changePassword")}
        </Button>
      )}
    </div>
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
  /** A tag chip was clicked: show the items with that tag. */
  onOpenTag: (tag: string) => void;
  /** "Datei anhängen …" is running (the picked file is read and encrypted in the backend). */
  attaching: boolean;
  onAddAttachment: () => void;
  onSaveAttachment: (attachment: AttachmentListEntry) => void;
  onRemoveAttachment: (attachment: AttachmentListEntry) => void;
  /** The vault cannot be changed here (the phone): no favourite toggle, edit, trash or attachment changes. */
  readOnly: boolean;
  /** "Speichern unter …" for attachments (a native save dialog: desktop only). */
  canSaveAttachments: boolean;
  /** Phone layout: back to the list (an arrow in front of the title). */
  onBack?: () => void;
}

export function ItemView({
  item,
  folders,
  onEdit,
  onTrash,
  onRestore,
  onDeleteForever,
  onToggleFavorite,
  onOpenTag,
  attaching,
  onAddAttachment,
  onSaveAttachment,
  onRemoveAttachment,
  readOnly,
  canSaveAttachments,
  onBack,
}: ItemViewProps) {
  const { t, tp, formatDateTime, formatRelative } = useT();
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLButtonElement>(null);
  const folder = folders.find((f) => f.id === item.folderId);
  const trashed = item.deletedAt !== null;

  return (
    <div className="detail" tabIndex={-1} data-detail-root>
      <header className="detail-header">
        {onBack && (
          <button type="button" className="icon-btn detail-back" onClick={onBack} aria-label={t("common.back")} title={t("common.back")}>
            <ArrowLeft />
          </button>
        )}
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
            {readOnly && item.favorite && !trashed && (
              // Read-only: the star is no button, so the chip says it.
              <span className="chip">
                <Star />
                {t("item.favorite")}
              </span>
            )}
            {folder && (
              <span className="chip">
                <FolderIcon />
                {folder.name}
              </span>
            )}
            {item.tags.map((tag) =>
              trashed ? (
                // The tag filter never shows the trash (a tag only the trash
                // carries has no filter at all): plain chips, like the folder.
                <span key={tag} className="chip">
                  <Tag />
                  {tag}
                </span>
              ) : (
                <button
                  key={tag}
                  type="button"
                  className="chip chip-btn"
                  onClick={() => onOpenTag(tag)}
                  title={t("tags.show", { name: tag })}
                  aria-label={t("tags.show", { name: tag })}
                >
                  <Tag />
                  {tag}
                </button>
              ),
            )}
            {item.attachments.length > 0 && (
              <span className="chip" title={tp("attachments.count", item.attachments.length)}>
                <Paperclip />
                {item.attachments.length}
                <span className="sr-only">{tp("attachments.count", item.attachments.length)}</span>
              </span>
            )}
          </div>
        </div>
        {!trashed && !readOnly && (
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
            {!readOnly && (
              <div className="trash-banner-actions">
                <button type="button" className="btn btn-secondary btn-sm" onClick={onRestore} data-detail-primary>
                  <RotateCcw />
                  {t("trash.restore")}
                </button>
                <button type="button" className="btn btn-danger-ghost btn-sm" onClick={onDeleteForever}>
                  {t("trash.deleteForever")}
                </button>
              </div>
            )}
          </div>
        )}

        {!trashed && item.type === "login" && <LeakWarning item={item} onEdit={readOnly ? undefined : onEdit} />}
        {item.type === "login" && <LoginBody item={item} />}
        {item.type === "card" && <CardBody item={item} />}
        {item.type === "identity" && <IdentityBody item={item} />}
        <Notes item={item} />
        <CustomFields item={item} />
        <Attachments
          item={item}
          attaching={attaching}
          vaultReadOnly={readOnly}
          canSave={canSaveAttachments}
          onAdd={onAddAttachment}
          onSave={onSaveAttachment}
          onRemove={onRemoveAttachment}
        />
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
