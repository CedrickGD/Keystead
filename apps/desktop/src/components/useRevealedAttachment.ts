import { api } from "../lib/api";
import { useRevealedValue, type RevealedSecret } from "./useRevealedSecret";

/**
 * The in-app preview of an image attachment (`reveal_attachment`): the data
 * URL is treated like a revealed secret (`useRevealedValue`) – it lives only
 * in the row's state and is dropped again after 30 s, when the item changes
 * (`revision` = its `updatedAt`), when the window loses focus or is hidden,
 * on lock and on unmount. The file bytes never reach the page otherwise.
 */
export function useRevealedAttachment(itemId: string, attachmentId: string, revision: number): RevealedSecret {
  return useRevealedValue(`${itemId}\n${attachmentId}\n${revision}`, () => api.revealAttachment(attachmentId));
}
