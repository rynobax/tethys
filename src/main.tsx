import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary, ErrorOverlay } from "./ErrorOverlay";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* Under the traffic lights, in place of the hidden native title bar. */}
    <div className="titlebar">Tethys</div>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
    <ErrorOverlay />
  </React.StrictMode>,
);
