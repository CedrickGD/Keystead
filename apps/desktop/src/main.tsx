import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { localPrefs } from "./lib/utils";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/components.css";
import "./styles/layout.css";
import "./styles/pages.css";
import "./styles/update.css";
import "./styles/import.css";

// Apply the last effective theme before the first paint (settings load async).
const cachedTheme = localPrefs.get("theme");
const prefersDark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
document.documentElement.dataset.theme =
  cachedTheme === "dark" || cachedTheme === "light" ? cachedTheme : prefersDark ? "dark" : "light";

// A desktop app should not show the browser's context menu (except in text fields).
window.addEventListener("contextmenu", (e) => {
  const target = e.target as HTMLElement | null;
  if (!target?.closest("input, textarea, .selectable")) e.preventDefault();
});

const container = document.getElementById("root");
if (!container) throw new Error("#root element missing");

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
