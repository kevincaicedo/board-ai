import { render } from "preact";
import { Stage } from "./app/Stage.tsx";
import "./styles/tokens.css";

const root = document.getElementById("app");
if (root === null) {
  throw new Error("index.html must contain #app");
}
render(<Stage />, root);
