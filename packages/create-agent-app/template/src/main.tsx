import React from "react";
import ReactDOM from "react-dom/client";
import "./actions"; // registers the agent actions as a side effect
import { App } from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
