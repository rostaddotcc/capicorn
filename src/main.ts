import "./style.css";
import { mountApp } from "./ui/app";

const root = document.getElementById("app");
if (root) {
  mountApp(root);
} else {
  console.error("Capicorn: #app mount point missing");
}