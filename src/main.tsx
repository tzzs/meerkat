import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { StoreProvider } from "./lib/store";
import { TooltipProvider } from "./components/ui/tooltip";
import "./styles/tailwind.css";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/window.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <StoreProvider>
      {/* One provider for the window: hovering a second tooltip straight after the first
          should not pay the delay again. */}
      <TooltipProvider delayDuration={350} skipDelayDuration={120}>
        <App />
      </TooltipProvider>
    </StoreProvider>
  </StrictMode>,
);
