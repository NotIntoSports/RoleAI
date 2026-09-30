import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "./styles/foundation.css";
import "./styles/shell.css";
import { App } from "./app/app";
import { I18nProvider } from "./i18n/I18nProvider";
import { initializeTheme } from "./features/appearance/theme";

const root = document.getElementById("root");
initializeTheme();

if (!root) {
  throw new Error("Tauri application root is missing");
}

createRoot(root).render(
  <StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </StrictMode>,
);
