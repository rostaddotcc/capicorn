import "./style.css";
import { applyTheme, readTheme } from "./ui/settings";
import { mountApp } from "./ui/app";

// index.html applies the stored theme before first paint; this keeps the module
// graph authoritative (and sets color-scheme on the root) once JS is live.
applyTheme(readTheme(), false);

const root = document.getElementById("app");
if (root) {
  mountApp(root);
} else {
  console.error("Capicorn: #app mount point missing");
}