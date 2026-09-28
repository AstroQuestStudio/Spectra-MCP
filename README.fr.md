# Spectra — serveur MCP navigateur natif en Rust

> Version française. English version: [README.md](README.md).

Spectra pilote Chrome directement via le Chrome DevTools Protocol (CDP), sans la couche d'abstraction de Playwright, pour donner à un agent IA (Claude Code ou tout client MCP) un contrôle de navigateur rapide, dense en information, et extensible par projet.

Voir [`docs/COMPARISON.md`](docs/COMPARISON.md) pour le comparatif chiffré face à Playwright MCP, mesuré sur une application réelle.

## Pourquoi

- **Binaire Rust natif** : ~27 Mo de RAM pour le process serveur (contre ~130 Mo pour un process Node.js équivalent), pas de runtime à installer.
- **CDP direct** : pas de double hop client → serveur-relais → navigateur.
- **Snapshot condensé** : arbre d'accessibilité élagué (rôles présentationnels retirés) avec des refs courtes `[eN]`, pas de screenshot comme seule source de vérité.
- **Extensible par projet** : chaque dépôt peut définir ses propres scénarios (`.spectra/recipes/*.mjs`) sans jamais recompiler le serveur.
- **Multi-session** : pilote plusieurs Chrome en parallèle (limite configurable), utile pour simuler du multi-utilisateur.
- **Core Web Vitals natifs** : LCP/CLS/FCP/TTFB mesurés via les APIs natives du navigateur (`PerformanceObserver`), sans dépendance externe (pas de Lighthouse, pas de web-vitals.js).
- **Sécurité par construction** : transport stdio uniquement (`rmcp` feature `transport-io`, jamais HTTP/SSE) — aucun socket réseau local n'est jamais ouvert, ce qui élimine structurellement toute la classe de vulnérabilité DNS rebinding qui a touché d'autres serveurs MCP navigateur exposant un transport HTTP local (ex. CVE-2025-9611 sur Playwright MCP). Rien à configurer, rien à auditer sur ce point : le vecteur d'attaque n'existe simplement pas dans cette architecture.

## Installation

### Prérequis

- Rust (édition 2021, toolchain stable) — `rustup` avec target `x86_64-pc-windows-msvc`
- Sur Windows : Visual Studio Build Tools avec le workload **Desktop development with C++** (le compilateur MSVC) **et** le composant **Windows SDK — Desktop C++ x64/x86** (contient `kernel32.lib` et les autres bibliothèques Win32 ; souvent absent d'une installation minimale des Build Tools, à ajouter explicitement)
- Node.js **22+** dans le PATH (uniquement nécessaire pour `browser_run_recipe`, dégrade gracieusement si absent) — le runtime des recipes (`recipe-runner.mjs`) utilise le `WebSocket` global natif de Node, stable seulement depuis la version 22 ; sur une version antérieure, `browser_run_recipe` échouerait avec `WebSocket is not defined`
- Google Chrome installé (détecté automatiquement dans les emplacements standards)

### Build

```powershell
cargo build --release
```

Le profil `release` utilise `lto = "thin"` plutôt que `lto = true` (fat LTO) — le LTO complet a fait planter LLVM par manque de mémoire sur une machine de développement standard pendant la mise au point de ce projet ; `thin` capture l'essentiel du gain de performance sans ce risque.

Le binaire final se trouve dans `target/release/spectra-server.exe`.

### Installation permanente

```powershell
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.spectra\bin"
Copy-Item "target\release\spectra-server.exe" "$env:USERPROFILE\.spectra\bin\spectra-server.exe"
```

## Activation dans Claude Code

Ajoute manuellement l'entrée suivante à ta configuration MCP (`~/.claude.json` ou équivalent selon ta plateforme, section `mcpServers`) :

```json
{
  "mcpServers": {
    "spectra": {
      "command": "%USERPROFILE%\\.spectra\\bin\\spectra-server.exe"
    }
  }
}
```

Une fois ajouté, `/mcp` dans Claude Code doit lister `spectra` avec ses 21 tools disponibles.

## Nettoyage d'urgence

Si des instances Chrome de test s'accumulent (ça arrive si un client MCP crashe sans terminer proprement le serveur) :

```powershell
powershell -File scripts\kill-all-spectra.ps1
```

Ce script cible précisément les process Chrome lancés par Spectra (identifiés par leur `--user-data-dir` sous `%LOCALAPPDATA%\Spectra\profiles`) — il ne touche jamais ton Chrome personnel.

Alternative depuis Claude Code, sans quitter la conversation :

```
browser_sessions(action="close_all")
```

## Variables d'environnement

| Variable | Rôle |
|---|---|
| `SPECTRA_MAX_SESSIONS` | Nombre maximum de sessions Chrome simultanées (défaut : 6) |
| `SPECTRA_CHROME_PATH` | Chemin explicite vers un binaire Chrome/Chromium, si Spectra ne le trouve pas automatiquement dans les emplacements standards |
| `SPECTRA_MINIMAL_CHROME` | Mettre à `0` pour désactiver les 7 flags de lancement additionnels (GPU, notifications, etc. — voir docs/COMPARISON.md, section V10) qui réduisent l'empreinte RAM de Chrome d'environ 4 %. Actifs par défaut |
| `SPECTRA_WINDOW_SIZE` | Résolution fenêtre + viewport CDP, format `"1920x1080"` (défaut : `1920x1080`, 16:9 dense) |
| `SPECTRA_PREWARM` | Mettre à `1` pour activer le pré-chauffage d'un Chrome dès le démarrage du serveur (TTL 20s si non consommé). Désactivé par défaut — aucun Chrome ne s'ouvre tant qu'un `browser_launch`/`browser_navigate` explicite n'est pas appelé |
| `SPECTRA_STEALTH` | Mettre à `1` pour désactiver les signaux de détection d'automatisation les plus visibles côté page (`navigator.webdriver`, bannière "Chrome est contrôlé", GPU désactivé) — pour piloter un site tiers qui bloque les navigateurs automatisés détectés. Désactivé par défaut (`SPECTRA_MINIMAL_CHROME` reste le mode normal, optimisé perf plutôt que discrétion). Combiner avec `ctx.addInitScript()` côté recipe pour un script d'évasion JS plus poussé (masquer `navigator.plugins`, WebGL fingerprint, etc.) |
| `SPECTRA_USER_AGENT` | User-agent custom (utile en mode stealth — un user-agent par défaut contenant `HeadlessChrome` est lui-même un signal de détection), sans effet si `SPECTRA_STEALTH` n'est pas activé |

## Catalogue des tools

| Tool | Description |
|---|---|
| `browser_launch` | Démarre ou rattache Chrome pour le projet courant |
| `browser_snapshot` | Capture l'arbre d'accessibilité condensé (texte + refs `[eN]`, avec état `[disabled]`/`[checked]`/`[expanded]`/... quand présent). Avec `diff_only=true`, ne retourne qu'un résumé compact des nœuds ajoutés/retirés/modifiés depuis le précédent snapshot de cette page (diff structurel par `backendDOMNodeId`, pas par texte de ligne). Avec `check_a11y=true`, ajoute une liste `a11y_issues` de problèmes d'accessibilité basiques détectés. Avec `detail="compact"` ou `"refs_only"`, réduit la verbosité pour économiser des tokens (voir `docs/COMPARISON.md`, section V8) |
| `browser_navigate` | Navigue vers une URL, attend le montage applicatif |
| `browser_click` | Clique sur un élément référencé. Supporte `diff_only=true` |
| `browser_type` | Saisit du texte, option de soumission (Entrée). Supporte `diff_only=true` |
| `browser_fill_form` | Remplit plusieurs champs en un seul appel. Supporte `diff_only=true` |
| `browser_select_option` | Sélectionne une option de `<select>`. Supporte `diff_only=true` |
| `browser_hover` | Survole un élément. Supporte `diff_only=true` |
| `browser_screenshot` | Capture image (réservé aux vérifications visuelles) |
| `browser_wait_for` | Attend qu'un texte apparaisse et/ou qu'une ref désigne un élément réellement rendu (`DOM.getBoxModel` valide), avec timeout configurable |
| `browser_console_messages` | Messages console dédupliqués, avec compteur |
| `browser_network_requests` | Requêtes réseau groupées par domaine/statut |
| `browser_network_request_detail` | Détail complet d'une requête (headers, statut) |
| `browser_evaluate` | Exécute une expression JavaScript dans la page |
| `browser_performance_metrics` | Mesure les Core Web Vitals de la page active (LCP, CLS, FCP, TTFB) |
| `browser_tabs` | Liste/ouvre/ferme/sélectionne un onglet |
| `browser_file_upload` | Upload de fichiers vers un `<input type=file>` |
| `browser_run_recipe` | Exécute une recipe `.mjs` du projet courant |
| `browser_sessions` | Liste, ferme une, ou ferme toutes les sessions Chrome |
| `browser_report` | Compile le journal de la session (navigations, actions, erreurs) en un rapport structuré |
| `browser_act_sequence` | Exécute une séquence d'actions (`click`/`type`/`select_option`) en un seul appel, sans repasser par un tour de raisonnement entre chaque étape. Avec `diff_only=true`, ajoute un résumé compact du changement (capturé une fois, après la dernière étape) |

## Extensibilité par projet

Crée `<ton-projet>/.spectra/recipes/<nom>.mjs` :

```js
export default async function loginAdmin(ctx, args) {
  await ctx.navigate(`${process.env.SPECTRA_BASE_URL}/login`);
  const snap = await ctx.snapshot();

  const emailRef = ctx.findRef(snap, { role: "textbox", name: /email/i });
  await ctx.type(emailRef, args.email);

  const pwRef = ctx.findRef(snap, { role: "textbox", name: /mot de passe|password/i });
  await ctx.type(pwRef, args.password, { submit: true });

  await ctx.waitFor({ text: "Tableau de bord", timeoutMs: 8000 });
  return { loggedIn: true };
}
```

Appel depuis Claude Code :

```
browser_run_recipe(project="mon-projet", recipe="login_admin", args={email: "test@exemple.fr", password: "..."})
```

Le runtime Node (`recipe-runner.mjs`, un client CDP minimal sans dépendance npm) est embarqué dans le binaire et auto-installé dans `%LOCALAPPDATA%\Spectra\runtime\` au premier appel — rien à installer manuellement.

Secrets : passe-les explicitement dans `args` de `browser_run_recipe` (pas de mécanisme `.env`/`.gitignore` automatique pour l'instant).

## Gestion multi-session

Par défaut, jusqu'à **6 sessions Chrome simultanées** (configurable via `SPECTRA_MAX_SESSIONS`), une par clé de projet. Un second `browser_launch` sur le même projet réutilise toujours la session existante plutôt que d'en spawn une nouvelle.

Utile pour piloter plusieurs navigateurs en parallèle — par exemple simuler plusieurs utilisateurs testant une édition collaborative sur une même application.

**Comment basculer entre sessions** : à part `browser_launch` et `browser_run_recipe`, aucun tool n'accepte de paramètre `project` explicite — tous opèrent sur le "projet par défaut" du serveur, celui du dernier `browser_launch(project=...)` appelé. Pour agir alternativement sur deux sessions (`user_a`, `user_b`), rappelle `browser_launch(project=...)` avant chaque bloc d'actions ciblant l'une ou l'autre :

```
browser_launch(project="user_a") → browser_navigate(...) → browser_click(...)
browser_launch(project="user_b") → browser_navigate(...) → browser_click(...)
browser_launch(project="user_a") → browser_snapshot(...)   # reprend l'état de user_a, inchangé pendant l'intervalle
```

Chaque session garde son propre état (page, refs, historique) indépendamment de laquelle est "active" au niveau du serveur — vérifié en conditions réelles : naviguer `user_b` vers une autre page n'affecte jamais l'état de `user_a`, même après plusieurs allers-retours.

## Pré-chauffage au démarrage

Avec `SPECTRA_PREWARM=1`, dès le lancement du serveur, Spectra pré-chauffe silencieusement un Chrome pour le répertoire courant (le projet le plus probable) sans bloquer le handshake MCP. Si le premier `browser_launch`/`browser_navigate` cible ce même projet, il récupère instantanément ce Chrome déjà démarré. S'il n'est jamais consommé, il se ferme automatiquement après 20 secondes — aucun risque d'accumulation silencieuse.

## Architecture

```
crates/
├── spectra-cdp/       # Client CDP (launcher, session, snapshot, actions, observers réseau/console)
├── spectra-tools/      # Tools MCP exposés (schémas, routage rmcp)
└── spectra-server/     # Binaire, transport stdio MCP
runtime/
└── recipe-runner.mjs  # Client CDP JS minimal (embarqué dans le binaire via include_str!, auto-installé)
scripts/
└── kill-all-spectra.ps1  # Nettoyage d'urgence des Chrome de test
```

Bibliothèques principales : `chromiumoxide` (client CDP), `rmcp` (SDK MCP officiel Rust), `tokio` (runtime async).

## Historique du développement et limitations connues

Voir [`docs/COMPARISON.md`](docs/COMPARISON.md) — comparatif chiffré face à Playwright MCP et récit honnête de chaque vague de développement (V1 à aujourd'hui), y compris les tentatives abandonnées et les bugs trouvés en testant activement contre une application réelle.

## Licence

Gratuit à utiliser, y compris commercialement. Interdit de l'utiliser pour construire un produit ou service concurrent. Contributions bienvenues. Voir [LICENSE](LICENSE) (PolyForm Shield 1.0.0). Source-available, pas open source.
