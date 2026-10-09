import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { PanelApp } from "./panel/PanelApp";
import { StoreProvider } from "./lib/store";
import { TooltipProvider } from "./components/ui/tooltip";
import "./styles/tailwind.css";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/window.css";
import "./styles/panel.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <StoreProvider>
      <TooltipProvider delayDuration={350} skipDelayDuration={120}>
        <PanelApp />
      </TooltipProvider>
    </StoreProvider>
  </StrictMode>,
);
