import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { AppModelProvider } from "./state/AppModel";
import { CodexUsageProvider } from "./state/CodexUsage";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AppModelProvider>
      <CodexUsageProvider>
        <App />
      </CodexUsageProvider>
    </AppModelProvider>
  </StrictMode>,
);
