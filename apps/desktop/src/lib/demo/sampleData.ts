// Realistic sample content for the mock backend ("Privat" and "Arbeit",
// master password "demo"). Includes weak, reused and old passwords so the
// security report has something to show.

import type {
  CardData,
  CustomField,
  Folder,
  GeneratedPassword,
  IdentityData,
  LoginData,
  PasswordHistoryEntry,
  VaultItem,
} from "../types";

const DAY = 86_400_000;

interface Base {
  id: string;
  name: string;
  folderId?: string | null;
  favorite?: boolean;
  notes?: string;
  fields?: CustomField[];
  history?: PasswordHistoryEntry[];
  /** Days since creation. */
  created: number;
  /** Days since last update (defaults to `created`). */
  updated?: number;
}

function base(now: number, b: Base): Omit<VaultItem, "type" | "login" | "card" | "identity"> {
  return {
    id: b.id,
    name: b.name,
    folderId: b.folderId ?? null,
    favorite: b.favorite ?? false,
    notes: b.notes ?? "",
    fields: b.fields ?? [],
    passwordHistory: b.history ?? [],
    createdAt: now - b.created * DAY,
    updatedAt: now - (b.updated ?? b.created) * DAY,
    deletedAt: null,
  };
}

function login(
  now: number,
  b: Base & { username: string; password: string; uris?: string[]; totp?: string; revised?: number },
): VaultItem {
  const data: LoginData = {
    username: b.username,
    password: b.password,
    uris: (b.uris ?? []).map((uri) => ({ uri, match: "domain" as const })),
    totp: b.totp ?? "",
    passwordRevisedAt: b.revised === undefined ? null : now - b.revised * DAY,
  };
  return { ...base(now, b), type: "login", login: data, card: null, identity: null };
}

function card(now: number, b: Base & { card: CardData }): VaultItem {
  return { ...base(now, b), type: "card", login: null, card: b.card, identity: null };
}

function identity(now: number, b: Base & { identity: Partial<IdentityData> }): VaultItem {
  const empty: IdentityData = {
    title: "",
    firstName: "",
    lastName: "",
    email: "",
    phone: "",
    company: "",
    address1: "",
    address2: "",
    postalCode: "",
    city: "",
    state: "",
    country: "",
    username: "",
  };
  return { ...base(now, b), type: "identity", login: null, card: null, identity: { ...empty, ...b.identity } };
}

function note(now: number, b: Base): VaultItem {
  return { ...base(now, b), type: "note", login: null, card: null, identity: null };
}

export interface SampleVault {
  name: string;
  items: VaultItem[];
  folders: Folder[];
  generatorHistory: GeneratedPassword[];
  createdDaysAgo: number;
  hasRecoveryKey: boolean;
}

export function privateVault(now: number): SampleVault {
  const fin = "f-finanzen";
  const soc = "f-social";
  const shop = "f-shopping";
  const mail = "max.mustermann@gmail.com";
  const items: VaultItem[] = [
    login(now, {
      id: "p-amazon", name: "Amazon", username: mail, password: "Sommer2019!", folderId: shop, favorite: true,
      uris: ["https://www.amazon.de"], created: 820, updated: 40,
    }),
    login(now, {
      id: "p-google", name: "Google", username: mail, password: "vT7#qLm2!xR9pZ4wKe", favorite: true,
      uris: ["https://accounts.google.com"], totp: "JBSWY3DPEHPK3PXP", created: 900, updated: 12, revised: 60,
      fields: [{ name: "Wiederherstellungs-E-Mail", value: "erika.mustermann@web.de", kind: "text" }],
    }),
    login(now, {
      id: "p-github", name: "GitHub", username: "maxmuster", password: "gh$Wq9!nVb3@Lr7tYp2", uris: ["https://github.com/login"],
      totp: "otpauth://totp/GitHub:maxmuster?secret=KRSXG5CTMVRXEZLUKN2XAZLSKNSWG4TFOQ&issuer=GitHub&algorithm=SHA1&digits=6&period=30",
      created: 640, updated: 3, revised: 3,
      fields: [{ name: "Wiederherstellungscodes", value: "8f2a-77c1 · 19bd-4e0a · c3d9-a612", kind: "hidden" }],
      history: [
        { password: "Github2021!", replacedAt: now - 3 * DAY },
        { password: "maxmuster1984", replacedAt: now - 400 * DAY },
      ],
    }),
    login(now, {
      id: "p-netflix", name: "Netflix", username: mail, password: "Sommer2019!", uris: ["https://www.netflix.com"],
      created: 760,
    }),
    login(now, {
      id: "p-sparkasse", name: "Sparkasse Online-Banking", username: "MM47110815", password: "K9#vR2m!Lp8@sQ4x",
      folderId: fin, favorite: true, uris: ["https://www.sparkasse.de"], created: 1200, updated: 30, revised: 30,
      notes: "Legitimations-ID steht auf dem Brief vom März. TAN-Verfahren: S-pushTAN.",
    }),
    login(now, {
      id: "p-paypal", name: "PayPal", username: mail, password: "pP!7xQz#4mNw2Lk9Rt", folderId: fin,
      uris: ["https://www.paypal.com/signin"], totp: "MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43U", created: 980, updated: 90, revised: 90,
    }),
    login(now, {
      id: "p-instagram", name: "Instagram", username: "max.muster", password: "hallo123", folderId: soc,
      uris: ["https://www.instagram.com"], created: 500, updated: 500,
    }),
    login(now, {
      id: "p-facebook", name: "Facebook", username: mail, password: "Sommer2019!", folderId: soc,
      uris: ["https://www.facebook.com"], created: 1500, updated: 700,
    }),
    login(now, {
      id: "p-linkedin", name: "LinkedIn", username: mail, password: "Ln#8vB2!qZ5mX7rT", folderId: soc,
      uris: ["https://www.linkedin.com/login"], created: 300, updated: 300,
    }),
    login(now, {
      id: "p-zalando", name: "Zalando", username: mail, password: "zalando2020", folderId: shop,
      uris: ["https://www.zalando.de"], created: 1100,
    }),
    login(now, {
      id: "p-ebay", name: "eBay", username: "maxm_84", password: "eB@y-Qr7!vK2#pL9", folderId: shop,
      uris: ["https://signin.ebay.de"], created: 1300, updated: 600,
    }),
    login(now, {
      id: "p-bahn", name: "Deutsche Bahn", username: mail, password: "Zug#Fahrt7!Rhein2Mosel", favorite: true,
      uris: ["https://www.bahn.de"], created: 200, updated: 20,
      fields: [{ name: "BahnCard-Nummer", value: "7081 4112 3456 7890", kind: "text" }],
    }),
    login(now, {
      id: "p-spotify", name: "Spotify", username: mail, password: "S9!pot#Fy2xQ7mLw", uris: ["https://accounts.spotify.com"],
      created: 450, updated: 45,
    }),
    login(now, {
      id: "p-fritzbox", name: "FRITZ!Box 7590", username: "", password: "Fr1tz!Box#Heim2024", uris: ["http://fritz.box"],
      created: 380, updated: 380,
      fields: [
        { name: "WLAN-Name", value: "Mustermann-WLAN", kind: "text" },
        { name: "WLAN-Passwort", value: "8271-4410-9932-0175", kind: "hidden" },
        { name: "Gastnetz aktiv", value: "true", kind: "boolean" },
      ],
    }),
    login(now, {
      id: "p-steam", name: "Steam", username: "maxgamer84", password: "passwort", uris: ["https://store.steampowered.com/login"],
      created: 1400,
    }),
    login(now, {
      id: "p-dropbox", name: "Dropbox", username: mail, password: "Dr0p!box#Kq8vM2xZ", uris: ["https://www.dropbox.com/login"],
      created: 1000, updated: 150, revised: 150,
      history: [{ password: "dropbox2019", replacedAt: now - 150 * DAY }],
    }),
    login(now, {
      id: "p-microsoft", name: "Microsoft-Konto", username: mail, password: "Ms#9vQ2!xLp7Rk4w", favorite: false,
      uris: ["https://login.live.com"], totp: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", created: 700, updated: 75, revised: 75,
    }),
    login(now, {
      id: "p-telekom", name: "Telekom Kundencenter", username: "max.mustermann@t-online.de", password: "T3l#kom!8vQ2xMz",
      uris: ["https://www.telekom.de/kundencenter"], created: 260,
    }),
    card(now, {
      id: "p-visa", name: "DKB Visa", folderId: fin, favorite: true, created: 600, updated: 200,
      card: { cardholderName: "Max Mustermann", brand: "Visa", number: "4111111111111111", expMonth: "08", expYear: "2028", code: "737" },
      notes: "Sperr-Hotline: 116 116",
    }),
    card(now, {
      id: "p-mastercard", name: "Barclays Mastercard", folderId: fin, created: 420,
      card: { cardholderName: "Max Mustermann", brand: "Mastercard", number: "5555555555554444", expMonth: "03", expYear: "2027", code: "214" },
    }),
    card(now, {
      id: "p-girocard", name: "Sparkasse Girocard", folderId: fin, created: 1200,
      card: { cardholderName: "Max Mustermann", brand: "Maestro", number: "6759649826438453", expMonth: "12", expYear: "2026", code: "" },
      fields: [{ name: "PIN", value: "4821", kind: "hidden" }],
    }),
    identity(now, {
      id: "p-id-max", name: "Max Mustermann", favorite: false, created: 900, updated: 100,
      identity: {
        title: "Herr", firstName: "Max", lastName: "Mustermann", email: mail, phone: "+49 170 1234567",
        address1: "Musterstraße 12", postalCode: "10115", city: "Berlin", state: "Berlin", country: "Deutschland",
        username: "maxmuster",
      },
    }),
    identity(now, {
      id: "p-id-erika", name: "Erika Mustermann", created: 340,
      identity: {
        title: "Frau", firstName: "Erika", lastName: "Mustermann", email: "erika.mustermann@web.de", phone: "+49 171 7654321",
        company: "Musterfirma GmbH", address1: "Musterstraße 12", postalCode: "10115", city: "Berlin", country: "Deutschland",
      },
    }),
    note(now, {
      id: "p-note-pass", name: "Reisepass", created: 800, updated: 210,
      notes: "Reisepass-Nr.: C01X00T47\nAusgestellt: 14.03.2021, Bürgeramt Mitte\nGültig bis: 13.03.2031",
    }),
    note(now, {
      id: "p-note-licenses", name: "Software-Lizenzen", created: 520, updated: 15,
      notes: "Office 2021 Home: XXXXX-7HQ2K-…\nAffinity Photo: AP2-4471-0099\nWindows 11 Pro: an Gerät gebunden",
    }),
    note(now, {
      id: "p-note-wifi", name: "WLAN für Gäste", folderId: null, created: 120,
      notes: "Netzwerk: Mustermann-Gast\nPasswort: Kaffee-und-Kuchen-2024",
    }),
  ];

  const trashed = login(now, {
    id: "p-myspace", name: "MySpace", username: "maxi1984", password: "maxi1984", uris: ["https://myspace.com"], created: 3000,
  });
  trashed.deletedAt = now - 4 * DAY;
  items.push(trashed);

  return {
    name: "Privat",
    items,
    folders: [
      { id: fin, name: "Finanzen" },
      { id: shop, name: "Shopping" },
      { id: soc, name: "Soziale Netzwerke" },
    ],
    generatorHistory: [
      { password: "Zug#Fahrt7!Rhein2Mosel", createdAt: now - 20 * DAY },
      { password: "gh$Wq9!nVb3@Lr7tYp2", createdAt: now - 3 * DAY },
      { password: "Maple-Harbor-Comet7-Velvet-Tundra", createdAt: now - 2 * DAY },
    ],
    createdDaysAgo: 1500,
    hasRecoveryKey: true,
  };
}

export function workVault(now: number): SampleVault {
  const srv = "f-server";
  const tools = "f-tools";
  const mail = "m.mustermann@musterfirma.de";
  return {
    name: "Arbeit",
    items: [
      login(now, {
        id: "w-jira", name: "Jira", username: mail, password: "J!ra#2024vQx7Lm", folderId: tools,
        uris: ["https://musterfirma.atlassian.net"], created: 300, favorite: true,
      }),
      login(now, {
        id: "w-gitlab", name: "GitLab", username: "mmustermann", password: "Gl#8qV!2xRm7Kp4", folderId: tools,
        uris: ["https://gitlab.musterfirma.de"], totp: "JBSWY3DPEHPK3PXP", created: 280,
      }),
      login(now, {
        id: "w-aws", name: "AWS Console", username: "m.mustermann", password: "Aws!9#xQv2Lm7Rk", folderId: srv,
        uris: ["https://console.aws.amazon.com"], totp: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", created: 200, favorite: true,
        fields: [{ name: "Account-ID", value: "4410-2291-7735", kind: "text" }],
      }),
      login(now, {
        id: "w-vpn", name: "Firmen-VPN", username: "mmustermann", password: "Winter2023!", folderId: srv,
        uris: ["https://vpn.musterfirma.de"], created: 700,
      }),
      login(now, {
        id: "w-slack", name: "Slack", username: mail, password: "Winter2023!", folderId: tools,
        uris: ["https://musterfirma.slack.com"], created: 640,
      }),
      card(now, {
        id: "w-card", name: "Firmenkreditkarte", created: 150,
        card: { cardholderName: "Max Mustermann", brand: "American Express", number: "378282246310005", expMonth: "05", expYear: "2029", code: "4421" },
      }),
      note(now, {
        id: "w-note", name: "Notfallkontakte IT", created: 90,
        notes: "IT-Hotline: +49 30 1234-500\nBereitschaft (24/7): +49 160 9988776\nTicket-System: https://help.musterfirma.de",
      }),
    ],
    folders: [
      { id: srv, name: "Server" },
      { id: tools, name: "Tools" },
    ],
    generatorHistory: [],
    createdDaysAgo: 720,
    hasRecoveryKey: false,
  };
}

/** Items "found" in a legacy VaultX 1.x vault by the mock import. */
export function legacyImportItems(now: number): VaultItem[] {
  return [
    login(now, { id: "", name: "Web.de", username: "max.mustermann@web.de", password: "Webde!2018", uris: ["https://web.de"], created: 2000 }),
    login(now, { id: "", name: "Ebay Kleinanzeigen", username: "max.mustermann@web.de", password: "Kl3in#Anz!7q", uris: ["https://www.kleinanzeigen.de"], created: 1600 }),
    login(now, {
      id: "", name: "Router", username: "admin", password: "admin", uris: ["http://192.168.178.1"], created: 2200,
      fields: [{ name: "Telefon", value: "+49 30 555 123", kind: "text" }],
    }),
    login(now, { id: "", name: "Stadtwerke Kundenportal", username: "kd-449120", password: "Strom&Gas2021", uris: ["https://portal.stadtwerke.de"], created: 1700 }),
  ];
}
