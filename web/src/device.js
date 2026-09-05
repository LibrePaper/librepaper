import { mount } from "svelte";
import "./styles/app.css";
import Device from "./components/Device.svelte";

// Mounted into <body> rather than into a wrapper, for the same reason the
// sign-in page is: the stylesheet addresses the bar as `body > nav`.
mount(Device, { target: document.body });
