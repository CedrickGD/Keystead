// Types of layout.mjs (see there).

export declare const PHONE_MAX_WIDTH: number;
export declare const PHONE_MEDIA_QUERY: string;

export declare function isPhonePlatform(platform: string | null | undefined): boolean;

export declare function isNarrowViewport(width: number | null | undefined): boolean;

export declare function isPhoneLayout(platform: string | null | undefined, narrow: boolean): boolean;

export interface PhoneBackState {
  /** The item editor is open. */
  editing: boolean;
  /** An item's detail view is open (stacked over the list). */
  detailOpen: boolean;
  /** The search field holds a query. */
  searching: boolean;
  /** A page other than the item list is shown (settings, security report, generator). */
  page: boolean;
  /** The item list shows a filter other than "all items". */
  filtered: boolean;
}

export type PhoneBackStep = "editor" | "detail" | "search" | "home" | null;

export declare function phoneBackStep(state: PhoneBackState): PhoneBackStep;
