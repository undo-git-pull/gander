import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import App from "./App";
import { initStore } from "./store";
import { loadLayouts } from "./lib/layout";

async function boot() {
  const root = createRoot(document.getElementById("root")!);
  try {
    await Promise.all([initStore(), loadLayouts()]);
    root.render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
  } catch (e) {
    root.render(<pre style={{ padding: 8, color: "crimson", whiteSpace: "pre-wrap" }}>Gander failed to start: {String(e)}</pre>);
  }
}

void boot();
