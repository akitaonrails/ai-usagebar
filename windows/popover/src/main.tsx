import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import App from "./App";
import "./index.css";

/** Hover hints wait a moment, so sweeping the pointer across a list does not flash them; moving
 * from one hint to the next is still instant (Radix's skipDelayDuration). They hold nothing to
 * click, so a hint closes as soon as the pointer leaves its element (disableHoverableContent). */
const HINT_DELAY_MS = 500;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <TooltipProvider delayDuration={HINT_DELAY_MS} disableHoverableContent>
      <App />
    </TooltipProvider>
  </StrictMode>,
);
