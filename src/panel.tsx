import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { PanelApp } from "./panel/PanelApp";
import { StoreProvider } from "./lib/store";
import "./styles/tailwind.css";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/window.css";
import "./styles/panel.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <StoreProvider>
      <PanelApp />
    </StoreProvider>
  </StrictMode>,
);
