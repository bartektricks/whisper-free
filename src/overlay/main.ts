import { mount } from "svelte";
import "./overlay.css";
import Overlay from "./Overlay.svelte";

const target = document.getElementById("overlay");
if (!target) throw new Error("#overlay mount point is missing from overlay.html");

export default mount(Overlay, { target });
