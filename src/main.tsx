import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Feel like a desktop app: no browser right-click menu outside text fields.
document.addEventListener("contextmenu", (e) => {
  const target = e.target as HTMLElement;
  if (!target.closest("input, textarea")) e.preventDefault();
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
