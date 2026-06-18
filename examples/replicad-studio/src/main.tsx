import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");
// No <StrictMode>: its dev double-invoke creates then disposes the WebGL context, and the
// second mount can't reacquire it — leaving a blank canvas. The three.js setup is idempotent
// in production anyway.
createRoot(root).render(<App />);
