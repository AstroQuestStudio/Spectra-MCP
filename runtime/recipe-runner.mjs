#!/usr/bin/env node
// Exécute une recipe .mjs d'un projet Spectra en pilotant Chrome via CDP brut
// (WebSocket natif de Node 22+, aucune dépendance npm — cohérent avec le
// principe "pas de runtime Node permanent, pas d'install à gérer par projet"
// documenté dans le README). Contrat d'appel (voir crates/spectra-tools/src/recipe.rs) :
//   stdin  : JSON { cdp_ws_url, project_dir, recipe_path, args }
//   stdout : JSON (résultat retourné par la fonction par défaut de la recipe)
// Toute erreur sort avec un code non-zéro et un message sur stderr — le
// process Rust appelant les distingue déjà (bail! sur output.status non-success).

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf-8");
}

// Client CDP minimal par-dessus le WebSocket natif : un id de requête par
// appel, une Map de résolveurs en attente, et un multiplexage par sessionId
// pour cibler une page précise une fois attaché dessus (Target.attachToTarget).
function createCdpClient(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let nextId = 1;
  const pending = new Map();
  const eventListeners = new Map(); // method -> Set<fn>

  const ready = new Promise((resolve, reject) => {
    ws.addEventListener("open", () => resolve());
    ws.addEventListener("error", (e) => reject(new Error(`WebSocket CDP: ${e.message || e}`)));
  });

  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id !== undefined && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message || "CDP error"));
      else resolve(msg.result);
    } else if (msg.method) {
      const listeners = eventListeners.get(msg.method);
      if (listeners) for (const fn of listeners) fn(msg.params, msg.sessionId);
    }
  });

  async function send(method, params = {}, sessionId = undefined) {
    await ready;
    const id = nextId++;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify(payload));
    });
  }

  function on(method, fn) {
    if (!eventListeners.has(method)) eventListeners.set(method, new Set());
    eventListeners.get(method).add(fn);
  }

  function close() {
    try { ws.close(); } catch {}
  }

  return { send, on, close };
}

// Récupère la liste des pages via l'API HTTP DevTools (dérivée de l'URL
// WebSocket browser-level : ws://host:port/devtools/browser/<id> — le port
// est le même que /json/list en HTTP), pour attacher une session CDP sur la
// première page réelle plutôt que de rester au niveau Browser (qui ne peut
// pas naviguer/évaluer directement sur une page).
async function fetchFirstPageTargetId(wsUrl) {
  const match = wsUrl.match(/^ws:\/\/([^/]+)\//);
  if (!match) throw new Error(`URL CDP inattendue: ${wsUrl}`);
  const host = match[1];
  const resp = await fetch(`http://${host}/json/list`);
  const targets = await resp.json();
  const page = targets.find((t) => t.type === "page");
  if (!page) throw new Error("aucune page ouverte trouvée sur cette session Chrome");
  return page.id;
}

function makeRef(index) {
  return `e${index}`;
}

const NOISE_ROLES = new Set(["generic", "none", "presentation", "InlineTextBox"]);
const INTERACTIVE_ROLES = new Set([
  "button", "link", "textbox", "checkbox", "radio", "combobox",
  "listbox", "option", "menuitem", "tab", "switch", "slider", "searchbox",
]);

// Capture un arbre d'accessibilité condensé équivalent au format Rust
// (refs [eN], élagage des rôles présentationnels) — implémentation JS
// volontairement simplifiée (pas de compression de listes répétitives ni de
// diff structurel, qui restent l'apanage des tools natifs `browser_snapshot`)
// : une recipe a surtout besoin de trouver une ref pour agir dessus, pas de
// la même densité d'affichage qu'un agent qui lit le texte lui-même.
async function captureSnapshot(cdp, sessionId) {
  const { nodes } = await cdp.send("Accessibility.getFullAXTree", {}, sessionId);
  const byId = new Map(nodes.map((n) => [n.nodeId, n]));
  const root = nodes.find((n) => !n.parentId) || nodes[0];

  const lines = [];
  const refs = [];
  let index = 0;

  function role(n) {
    return n.role && n.role.value ? n.role.value : "";
  }
  function name(n) {
    return n.name && n.name.value ? n.name.value : "";
  }
  function isInteractive(n) {
    return INTERACTIVE_ROLES.has(role(n));
  }

  function renderNode(node, depth) {
    const isNoise = node.ignored || NOISE_ROLES.has(role(node));
    if (!isNoise) {
      const indent = "  ".repeat(depth);
      const r = role(node);
      const nm = name(node);
      if (isInteractive(node) || nm) {
        const ref = makeRef(index++);
        refs.push({ ref, backendDOMNodeId: node.backendDOMNodeId ?? -1 });
        lines.push(nm ? `${indent}[${ref}] ${r} "${nm}"` : `${indent}[${ref}] ${r}`);
      } else {
        lines.push(`${indent}${r}`);
      }
    }
    const nextDepth = isNoise ? depth : depth + 1;
    for (const childId of node.childIds || []) {
      const child = byId.get(childId);
      if (child) renderNode(child, nextDepth);
    }
  }

  if (root) renderNode(root, 0);
  return { text: lines.join("\n"), refs };
}

async function resolveRef(cdp, sessionId, refs, ref) {
  const entry = refs.find((r) => r.ref === ref);
  if (!entry) throw new Error(`ref inconnue: ${ref} (le snapshot a peut-être changé — rappelle ctx.snapshot())`);
  return entry.backendDOMNodeId;
}

async function centerOf(cdp, sessionId, backendNodeId) {
  try {
    await cdp.send("DOM.scrollIntoViewIfNeeded", { backendNodeId }, sessionId);
  } catch (e) {
    process.stderr.write(`[warn] scrollIntoViewIfNeeded a échoué: ${e.message}\n`);
  }
  await new Promise((r) => setTimeout(r, 50));
  const { model } = await cdp.send("DOM.getBoxModel", { backendNodeId }, sessionId);
  const quad = model.content;
  const cx = (quad[0] + quad[2] + quad[4] + quad[6]) / 4;
  const cy = (quad[1] + quad[3] + quad[5] + quad[7]) / 4;
  return [cx, cy];
}

function buildCtx(cdp, sessionId, state) {
  return {
    // Injecte un script exécuté avant tout script de la page, dès le tout
    // début du chargement (même mécanisme CDP que les observers Core Web
    // Vitals côté serveur Rust, voir observers.rs::evaluate_on_new_document)
    // — utile pour mocker un global (ex: window.__TAURI_INTERNALS__) avant
    // que le code applicatif ne teste sa présence. Doit être appelé AVANT
    // `navigate()` pour s'appliquer à la prochaine navigation ; n'affecte pas
    // la page déjà chargée au moment de l'appel.
    async addInitScript(js) {
      await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: js }, sessionId);
    },
    // Exécute une expression JavaScript dans le contexte de la page et
    // retourne sa valeur (doit être sérialisable JSON — mêmes contraintes que
    // `browser_evaluate` côté serveur Rust). Équivalent de `page.evaluate()`
    // de @playwright/test : permet de réutiliser telle quelle une logique
    // d'extraction/action déjà écrite en JS pur (querySelectorAll, .click()
    // DOM natif) sans avoir à la réécrire autour du système de refs [eN].
    async evaluate(expression) {
      const { result } = await cdp.send("Runtime.evaluate", { expression, returnByValue: true }, sessionId);
      if (result.subtype === "error") {
        throw new Error(`l'expression a levé une exception: ${result.description || "erreur inconnue"}`);
      }
      return result.value;
    },
    // Démarre la capture des exceptions JS non catchées et des console.error
    // de la page, dans un buffer local à cette recipe (indépendant du buffer
    // du serveur Rust, qui vit dans un process séparé). Idempotent : un
    // second appel ne ré-active pas Runtime.enable deux fois. Retourne une
    // fonction `getErrors()` à appeler après navigation/action pour lire
    // l'état courant du buffer — pattern équivalent à `page.on('pageerror')`/
    // `page.on('console')` de @playwright/test, mais explicite plutôt
    // qu'implicite (pas d'auto-listen dès l'ouverture de la page).
    async collectErrors() {
      if (state.errorBuffer) return { getErrors: () => state.errorBuffer.slice() };
      state.errorBuffer = [];
      await cdp.send("Runtime.enable", {}, sessionId);
      cdp.on("Runtime.exceptionThrown", (params) => {
        const desc = params.exceptionDetails?.exception?.description
          || params.exceptionDetails?.text
          || "exception JS sans message";
        state.errorBuffer.push({ type: "exception", text: desc });
      });
      cdp.on("Runtime.consoleAPICalled", (params) => {
        if (params.type !== "error") return;
        const text = (params.args || []).map((a) => a.value ?? a.description ?? "").join(" ");
        state.errorBuffer.push({ type: "console.error", text });
      });
      return { getErrors: () => state.errorBuffer.slice() };
    },
    // Démarre la capture des requêtes réseau (URL + statut de réponse) dans
    // un buffer local — équivalent recipe de `page.on('request'/'response')`
    // de @playwright/test. Idempotent comme `collectErrors`. Utile pour
    // détecter des patterns dangereux (ex: `company_id=undefined` dans une
    // URL Supabase/PostgREST) ou des statuts d'erreur (4xx/5xx) pendant un
    // parcours de navigation.
    async collectNetwork() {
      if (state.networkBuffer) return { getRequests: () => state.networkBuffer.slice() };
      state.networkBuffer = [];
      await cdp.send("Network.enable", {}, sessionId);
      const pendingUrls = new Map(); // requestId -> url
      cdp.on("Network.requestWillBeSent", (params) => {
        pendingUrls.set(params.requestId, params.request.url);
      });
      cdp.on("Network.responseReceived", (params) => {
        const url = params.response.url || pendingUrls.get(params.requestId) || "";
        state.networkBuffer.push({ url, status: params.response.status });
      });
      return { getRequests: () => state.networkBuffer.slice() };
    },
    async navigate(url) {
      const navigatedPromise = new Promise((resolve) => {
        cdp.on("Page.loadEventFired", () => resolve());
      });
      await cdp.send("Page.navigate", { url }, sessionId);
      // Attend le vrai évènement de chargement (Page.loadEventFired) avant de
      // commencer à poller le montage applicatif — sans ça, une première
      // mesure prise pendant que `about:blank` est encore affiché peut lire
      // un texte vide "stable" par coïncidence (deux mesures à 0 de suite) et
      // conclure à tort que la page est montée alors qu'elle n'a pas encore
      // commencé à charger.
      await Promise.race([navigatedPromise, new Promise((r) => setTimeout(r, 8000))]);
      await new Promise((resolve) => setTimeout(resolve, 300));

      let lastLen = null, stable = 0;
      const deadline = Date.now() + 4000;
      while (Date.now() < deadline) {
        const { result } = await cdp.send(
          "Runtime.evaluate",
          { expression: "document.body ? document.body.innerText.length : 0" },
          sessionId
        );
        const len = result.value ?? 0;
        if (len === lastLen && len > 0) {
          stable++;
          if (stable >= 2) break;
        } else {
          stable = 0;
        }
        lastLen = len;
        await new Promise((r) => setTimeout(r, 100));
      }
      return this.snapshot();
    },
    async snapshot() {
      const { text, refs } = await captureSnapshot(cdp, sessionId);
      state.refs = refs;
      return { text, refCount: refs.length };
    },
    findRef(snap, { role, name }) {
      if (!state.refs) throw new Error("appelle ctx.snapshot() avant ctx.findRef()");
      const lines = snap.text.split("\n");
      for (const line of lines) {
        const m = line.match(/\[(e\d+)\]\s+(\S+)(?:\s+"([^"]*)")?/);
        if (!m) continue;
        const [, ref, lineRole, lineName = ""] = m;
        if (role && lineRole !== role) continue;
        if (name) {
          const re = name instanceof RegExp ? name : new RegExp(name, "i");
          if (!re.test(lineName)) continue;
        }
        return ref;
      }
      throw new Error(`aucun élément trouvé pour role=${role} name=${name}`);
    },
    async click(ref) {
      const backendNodeId = await resolveRef(cdp, sessionId, state.refs, ref);
      const [x, y] = await centerOf(cdp, sessionId, backendNodeId);
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y }, sessionId);
      await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 }, sessionId);
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1 }, sessionId);
    },
    async type(ref, text, opts = {}) {
      const backendNodeId = await resolveRef(cdp, sessionId, state.refs, ref);
      await cdp.send("DOM.focus", { backendNodeId }, sessionId);
      for (const ch of text) {
        await cdp.send("Input.dispatchKeyEvent", { type: "char", text: ch }, sessionId);
      }
      if (opts.submit) {
        await cdp.send("Input.dispatchKeyEvent", { type: "rawKeyDown", windowsVirtualKeyCode: 13, key: "Enter" }, sessionId);
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", windowsVirtualKeyCode: 13, key: "Enter" }, sessionId);
      }
    },
    async waitFor({ text, timeoutMs = 5000 }) {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        if (text) {
          const { result } = await cdp.send(
            "Runtime.evaluate",
            { expression: "document.body ? document.body.innerText : ''" },
            sessionId
          );
          if ((result.value || "").includes(text)) return { satisfied: true };
        }
        await new Promise((r) => setTimeout(r, 150));
      }
      return { satisfied: false, timedOut: true };
    },
    // Simule un raccourci clavier global (ex: "Control+k", "Escape") — pour
    // des interactions qui ne ciblent pas un élément précis (contrairement à
    // `type`, qui focus d'abord une ref). Parse un combo façon Playwright
    // ("Control+k") en modificateurs CDP (bitmask Input.dispatchKeyEvent :
    // Alt=1, Ctrl=2, Meta/Cmd=4, Shift=8) + touche finale.
    async pressKey(combo) {
      const parts = combo.split("+");
      const keyPart = parts.pop();
      let modifiers = 0;
      for (const mod of parts) {
        const m = mod.toLowerCase();
        if (m === "alt") modifiers |= 1;
        else if (m === "control" || m === "ctrl") modifiers |= 2;
        else if (m === "meta" || m === "cmd") modifiers |= 4;
        else if (m === "shift") modifiers |= 8;
      }
      // Table minimale des touches spéciales utiles en pratique (raccourcis
      // globaux, fermeture de modale) — étendre au besoin plutôt que de
      // prétendre couvrir tout le clavier dès le départ.
      const KEY_CODES = {
        escape: { code: 27, key: "Escape" },
        enter: { code: 13, key: "Enter" },
        tab: { code: 9, key: "Tab" },
        backspace: { code: 8, key: "Backspace" },
        arrowup: { code: 38, key: "ArrowUp" },
        arrowdown: { code: 40, key: "ArrowDown" },
        arrowleft: { code: 37, key: "ArrowLeft" },
        arrowright: { code: 39, key: "ArrowRight" },
      };
      const lower = keyPart.toLowerCase();
      let windowsVirtualKeyCode, key;
      if (KEY_CODES[lower]) {
        ({ code: windowsVirtualKeyCode, key } = KEY_CODES[lower]);
      } else if (keyPart.length === 1) {
        windowsVirtualKeyCode = keyPart.toUpperCase().charCodeAt(0);
        key = keyPart;
      } else {
        throw new Error(`touche non reconnue dans pressKey: "${keyPart}" (combo: "${combo}")`);
      }
      await cdp.send(
        "Input.dispatchKeyEvent",
        { type: "rawKeyDown", modifiers, windowsVirtualKeyCode, key },
        sessionId
      );
      await cdp.send(
        "Input.dispatchKeyEvent",
        { type: "keyUp", modifiers, windowsVirtualKeyCode, key },
        sessionId
      );
    },
    // Clic à des coordonnées viewport explicites — pour cibler un point qui
    // n'est pas forcément un élément avec ref (ex: fermer une modale en
    // cliquant sur le backdrop, hors de tout élément nommé/interactif).
    async clickAt(x, y) {
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y }, sessionId);
      await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 }, sessionId);
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1 }, sessionId);
    },
  };
}

async function main() {
  const raw = await readStdin();
  const { cdp_ws_url, recipe_path, args } = JSON.parse(raw);

  const cdp = createCdpClient(cdp_ws_url);
  const targetId = await fetchFirstPageTargetId(cdp_ws_url);
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });

  await cdp.send("Page.enable", {}, sessionId);
  await cdp.send("DOM.enable", {}, sessionId);
  await cdp.send("Runtime.enable", {}, sessionId);
  await cdp.send("Accessibility.enable", {}, sessionId);
  await cdp.send("Page.bringToFront", {}, sessionId).catch(() => {});

  const state = { refs: null };
  const ctx = buildCtx(cdp, sessionId, state);

  const recipeUrl = `file://${recipe_path.replace(/\\/g, "/")}`;
  const mod = await import(recipeUrl);
  const recipeFn = mod.default;
  if (typeof recipeFn !== "function") {
    throw new Error(`${recipe_path} doit exporter une fonction par défaut (export default async function(ctx, args) {...})`);
  }

  const result = await recipeFn(ctx, args ?? {});
  cdp.close();
  process.stdout.write(JSON.stringify(result ?? null));
}

main().catch((err) => {
  process.stderr.write(String(err && err.stack ? err.stack : err));
  process.exit(1);
});
