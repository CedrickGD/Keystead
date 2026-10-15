// Website icons of the open vault (see "Website icons" in
// docs/ARCHITECTURE.md): loaded once per vault page and again whenever the
// backend stored new ones (`vault://icons`) or the settings cleared them.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, events, subscribeEffect } from "../lib/api";
import { isIconDataUrl } from "../lib/icons";

interface IconsContextValue {
  /** host → data URL */
  icons: Readonly<Record<string, string>>;
  /** Re-reads the icons (e.g. after "Gespeicherte Icons löschen"). */
  refresh: () => void;
}

const EMPTY: Readonly<Record<string, string>> = Object.freeze({});

const IconsContext = createContext<IconsContextValue>({ icons: EMPTY, refresh: () => undefined });

export function IconsProvider({ children }: { children: ReactNode }) {
  const [icons, setIcons] = useState<Readonly<Record<string, string>>>(EMPTY);
  const seq = useRef(0);

  const refresh = useCallback(() => {
    const mine = ++seq.current;
    api
      .getIcons()
      .then((map) => {
        if (mine !== seq.current) return; // a newer answer is on its way
        const clean: Record<string, string> = {};
        for (const [host, url] of Object.entries(map ?? {})) if (isIconDataUrl(url)) clean[host] = url;
        setIcons(clean);
      })
      // Locked or switched vault: the page is about to leave anyway.
      .catch(() => undefined);
  }, []);

  useEffect(() => refresh(), [refresh]);
  useEffect(() => subscribeEffect(events.onIconsChanged(refresh)), [refresh]);

  const value = useMemo(() => ({ icons, refresh }), [icons, refresh]);
  return <IconsContext.Provider value={value}>{children}</IconsContext.Provider>;
}

export function useIcons(): IconsContextValue {
  return useContext(IconsContext);
}

/** The stored icon (data URL) of a host, if any. */
export function useSiteIcon(host: string | null | undefined): string | undefined {
  const { icons } = useContext(IconsContext);
  return host ? icons[host] : undefined;
}
