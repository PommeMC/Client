import * as React from "react";
import * as ReactDOM from "react-dom/client";
import Router from "./Router";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Router />
  </React.StrictMode>,
);
