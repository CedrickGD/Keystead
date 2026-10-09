import { CreditCard, IdCard, StickyNote } from "lucide-react";
import type { ItemType } from "../lib/types";
import { avatarHue, avatarLetter } from "../lib/utils";

const TYPE_ICON: Partial<Record<ItemType, typeof CreditCard>> = {
  card: CreditCard,
  identity: IdCard,
  note: StickyNote,
};

/**
 * Item avatar. Logins get a softly tinted letter tile (the site's initial);
 * cards, identities and notes get a neutral tile with their type icon, so the
 * list reads at a glance without extra badges.
 */
export function Avatar({ name, type, size = "md" }: { name: string; type?: ItemType; size?: "sm" | "md" | "lg" }) {
  const Icon = type ? TYPE_ICON[type] : undefined;
  const sizeClass = size === "md" ? "" : size;
  if (Icon) {
    return (
      <div className={`avatar avatar-icon ${sizeClass}`} aria-hidden>
        <Icon />
      </div>
    );
  }
  return (
    <div className={`avatar ${sizeClass}`} style={{ ["--h" as string]: avatarHue(name) }} aria-hidden>
      {avatarLetter(name)}
    </div>
  );
}
