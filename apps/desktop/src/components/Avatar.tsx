import { CreditCard, IdCard, StickyNote } from "lucide-react";
import type { ItemType } from "../lib/types";
import { avatarHue, avatarLetter } from "../lib/utils";

const TYPE_BADGE: Partial<Record<ItemType, typeof CreditCard>> = {
  card: CreditCard,
  identity: IdCard,
  note: StickyNote,
};

/** Letter avatar tinted by a hash of the name; non-login items get a small type badge. */
export function Avatar({ name, type, size = "md" }: { name: string; type?: ItemType; size?: "sm" | "md" | "lg" }) {
  const Badge = type ? TYPE_BADGE[type] : undefined;
  return (
    <div
      className={`avatar ${size === "md" ? "" : size}`}
      style={{ ["--h" as string]: avatarHue(name) }}
      aria-hidden
    >
      {avatarLetter(name)}
      {Badge && size !== "sm" && (
        <span className="avatar-badge">
          <Badge />
        </span>
      )}
    </div>
  );
}
