import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { StoreProvider } from "./lib/store";
import { TooltipProvider } from "./components/ui/tooltip";
import { Guarded, installErrorReporting } from "./components/ErrorBoundary";
import "./styles/tailwind.css";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/window.css";

// Set before the first render rather than by App, so the page is marked even when App throws.
document.documentElement.dataset.pulse = "window";
installErrorReporting("window");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <StoreProvider>
      {/* One provider for the window: hovering a second tooltip straight after the first
          should not pay the delay again. */}
      <TooltipProvider delayDuration={350} skipDelayDuration={120}>
        <Guarded where="window">
          <App />
        </Guarded>
      </TooltipProvider>
    </StoreProvider>
  </StrictMode>,
);
