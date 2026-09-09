// Headless Firefox (BiDi) and Chromium (CDP) for local browser checks.
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";

export const pause = (ms) => new Promise((done) => setTimeout(done, ms));
export async function until(label, test, timeout = 60000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { if (await test()) return; } catch (error) { last = error; }
    await pause(150);
  }
  throw new Error(`${label} timed out${last ? `: ${last.message}` : ""}`);
}

function protocol(socket) {
  let serial = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(data);
    const job = pending.get(message.id);
    if (!job) return;
    pending.delete(message.id);
    clearTimeout(job.timer);
    message.error || message.type === "error"
      ? job.reject(new Error(JSON.stringify(message))) : job.resolve(message.result);
  });
  return (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++serial;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 60000);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
}

export async function browser(name, directory, port) {
  mkdirSync(directory, { recursive: true });
  const child = spawn(name, name === "firefox"
    ? ["--headless", "--no-remote", "--profile", directory, "--remote-debugging-port", String(port)]
    : ["--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", `--user-data-dir=${directory}`, `--remote-debugging-port=${port}`, "about:blank"],
  { stdio: "ignore" });
  let spawnError;
  child.on("error", (error) => { spawnError = error; });
  let socket;
  try {
    await until(`${name} startup`, async () => {
      if (spawnError) throw spawnError;
      const endpoint = name === "firefox" ? `ws://127.0.0.1:${port}/session`
        : (await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()).webSocketDebuggerUrl;
      socket = new WebSocket(endpoint);
      await new Promise((done, fail) => { socket.onopen = done; socket.onerror = fail; });
      return true;
    }, 20000);
    const send = protocol(socket);
    const close = async () => {
      socket.close();
      child.kill();
      await Promise.race([new Promise((done) => child.once("exit", done)), pause(2000)]);
    };
    if (name === "firefox") {
      await send("session.new", { capabilities: {} });
      const { context } = await send("browsingContext.create", { type: "tab" });
      const evaluate = async (expression, frame = context) => {
        const result = await send("script.evaluate", { expression, target: { context: frame }, awaitPromise: true });
        if (result.type === "exception") throw new Error(result.exceptionDetails.text);
        return result.result.value;
      };
      return {
        close, evaluate,
        setCookie: (name, value) => evaluate(`document.cookie = ${JSON.stringify(`${name}=${value}; path=/`)}`),
        navigate: (url) => send("browsingContext.navigate", { context, url, wait: "complete" }),
        frameEvaluate: async (expression) => {
          const tree = await send("browsingContext.getTree", { root: context });
          const frame = tree.contexts[0].children?.[0]?.context;
          return frame ? evaluate(expression, frame) : null;
        },
        text: async () => {
          const tree = await send("browsingContext.getTree", { root: context });
          const frame = tree.contexts[0].children?.[0]?.context;
          return frame ? evaluate("document.body.innerText", frame) : "";
        },
        insert: async (text, replace = false) => {
          await evaluate('document.querySelector(".cm-content").focus()');
          const down = (value) => ({ type: "keyDown", value });
          const up = (value) => ({ type: "keyUp", value });
          await send("input.performActions", { context, actions: [{ type: "key", id: "keyboard", actions: [
            down("\uE009"), down(replace ? "a" : "\uE011"), up(replace ? "a" : "\uE011"), up("\uE009"),
            ...Array.from(text).flatMap((c) => [down(c === "\n" ? "\uE007" : c), up(c === "\n" ? "\uE007" : c)]),
          ] }] });
        },
      };
    }
    const { targetId } = await send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
    const command = (method, params) => send(method, params, sessionId);
    await command("Page.enable");
    const evaluate = async (expression, contextId) => {
      const result = await command("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, ...(contextId ? { contextId } : {}) });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
      return result.result.value;
    };
    const frames = new Map();
    return {
      close, evaluate,
      setCookie: (name, value, url) => command("Network.setCookie", { name, value, url }),
      frameEvaluate: async (expression) => {
        const { targetInfos } = await send("Target.getTargets");
        const target = targetInfos.find((one) => one.type === "iframe");
        if (!target) return null;
        if (!frames.has(target.targetId)) frames.set(target.targetId, (await send("Target.attachToTarget", { targetId: target.targetId, flatten: true })).sessionId);
        const result = await send("Runtime.evaluate", { expression, returnByValue: true }, frames.get(target.targetId));
        if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
        return result.result.value;
      },
      resize: (width, height) => command("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false }),
      navigate: (url) => command("Page.navigate", { url }),
      text: async () => {
        const slug = await evaluate('location.pathname.split("/").pop()');
        const { targetInfos } = await send("Target.getTargets");
        const target = targetInfos.find((one) => one.type === "iframe" && one.url.includes(`/${slug}/`));
        if (target) {
          if (!frames.has(target.targetId)) frames.set(target.targetId, (await send("Target.attachToTarget", { targetId: target.targetId, flatten: true })).sessionId);
          const result = await send("Runtime.evaluate", { expression: "document.body.innerText", returnByValue: true }, frames.get(target.targetId));
          return result.result.value || "";
        }
        const { frameTree } = await command("Page.getFrameTree");
        const frameId = frameTree.childFrames?.[0]?.frame.id;
        if (!frameId) return "";
        const { executionContextId } = await command("Page.createIsolatedWorld", { frameId, worldName: "typst-check" });
        return evaluate("document.body.innerText", executionContextId);
      },
      insert: async (text, replace = false) => {
        const value = JSON.stringify(String(text));
        const inserted = await evaluate(`(() => {
          const editor = document.querySelector(".cm-content");
          if (!editor) throw new Error("missing CodeMirror editor");
          editor.focus();
          const selection = getSelection();
          selection.removeAllRanges();
          const range = document.createRange();
          range.selectNodeContents(editor);
          if (!${Boolean(replace)}) range.collapse(true);
          selection.addRange(range);
          return document.execCommand("insertText", false, ${value});
        })()`);
        if (inserted) return;
        // Some Chromium builds disable execCommand for a contenteditable
        // editor. Fall back to protocol key events, including explicit Enter
        // events so source newlines are retained.
        for (const [index, line] of String(text).split("\n").entries()) {
          if (line) await command("Input.insertText", { text: line });
          if (index + 1 < String(text).split("\n").length) {
            await command("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 });
            await command("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 });
          }
        }
      },
    };
  } catch (error) { socket?.close(); child.kill(); throw error; }
}
