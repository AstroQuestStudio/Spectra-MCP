> **English summary.** This document is a measured, end-to-end comparison of Spectra against Playwright MCP, written in French.
> Method: every figure was measured in the same session, on the same Windows 11 machine, against the same real application (a React/Vite SPA served on `localhost:8080`), over the real MCP stdio protocol; no synthetic benchmarks.
> Key results (Playwright MCP vs Spectra V26): snapshot of the reference page 334 lines / 8,666 bytes vs 114 lines / 3,513 bytes; server process RAM ~130 MB vs ~27 MB; Chrome headless RAM ~1.5 GB vs ~1.18 GB (-4.3%).
> Snapshot verbosity levels: `compact` -20.7%, `refs_only` -33.1%. Cold snapshot 79 ms vs ~32 ms; `navigate` 2,960 ms vs ~3.4 s (Spectra is slower there).
> The rest of the document (French) also records abandoned attempts and bugs found while testing.

# Spectra vs Playwright MCP — comparatif chiffré

Tous les chiffres ci-dessous ont été mesurés durant la même session, sur la même machine (Windows 11, Node 22.18, Rust 1.95 MSVC), contre la **même application réelle** : AstroQuest, une SPA React/Vite en mode développement, servie sur `localhost:8080`. Aucun benchmark synthétique — ce sont des mesures de bout en bout via le protocole MCP réel (stdio JSON-RPC), pas des micro-benchmarks internes.

## Vue d'ensemble — Playwright vs V1 vs dernière version (V26)

Le tableau ci-dessous compare Playwright MCP à Spectra à ses deux extrémités : **V1** (première version livrée) et **V26** (dernière version, état actuel). Le détail vague par vague (V2 à V25) reste disponible plus bas dans ce document pour qui veut l'historique complet — ici, seul l'écart de bout en bout compte.

| Dimension | Playwright MCP | V1 | V26 (actuel) |
|---|---|---|---|
| **Nombre de tools MCP** | ~25 (large surface, beaucoup redondants) | 17 | 21 |
| **`launch`** | 103 ms | 1 128 ms | **~0 ms*** (pool pré-chauffé) |
| **`navigate` (montage complet)** | 2 960 ms (`networkidle` strict, fragile) | 4 675 ms | ~3,4 s (détection DOM-settled adaptative) |
| **Snapshot à froid** | 79 ms | 52–96 ms | ~32 ms |
| **Taille snapshot (page repère, `detail=full`)** | 334 lignes / 8 666 o | 349 lignes | 114 lignes / 3 513 o |
| **Niveaux de verbosité configurables** | ✗ | ✗ | `full`/`compact` (−20,7 %)/`refs_only` (−33,1 %) |
| **Propriétés d'état exposées (disabled/checked/expanded...)** | Partiel (attributs ARIA bruts) | ✗ | 7 propriétés, whitelist stricte |
| **Diff après action** | ✗ (aucun mécanisme natif) | ✗ | Diff **structurel** par `backend_id` (click/type/hover/select/fill_form/act_sequence) + `state_flags` |
| **Faux négatif connu sur le diff** | N/A | Oui (non détecté) | Corrigé (voir V5) |
| **RAM process hôte (hors Chrome)** | ~130 Mo (Node.js) | ~27 Mo | ~27 Mo |
| **RAM Chrome headless** | ~1,5 Go (référence) | ~1,5 Go | **~1,18 Go** (−4,3 %, Chromium ultra-configuré, voir V10) |
| **Core Web Vitals (LCP/CLS/FCP/TTFB)** | ✗ | ✗ | Livré (`browser_performance_metrics`) |
| **Pool de Chrome pré-chauffé** | Oui (natif, mature) | ✗ | TTL 20s, testé plusieurs scénarios |
| **Gestion multi-session (multi-utilisateur)** | Limité | `SPECTRA_MAX_SESSIONS` (6 par défaut) | idem, + usage documenté (V13) |
| **Extensibilité par projet (recipes)** | ✗ | Documenté mais **cassé à 100%** (runtime jamais livré) | Corrigé (V9), testé bout en bout, auto-installation embarquée, vérification Node 22+ (V25) |
| **Gestion multi-onglets (`browser_tabs`)** | Fonctionnel | Documenté mais **`new`/`close`/`select` jamais implémentés** (seul `list` fonctionnait) | Corrigé (V11), testé 4 scénarios dont la fermeture de l'onglet actif |
| **Détail réseau (`browser_network_request_detail`)** | Fonctionnel | Documenté ("headers, statut") mais **headers jamais capturés** | Corrigé (V26) : `request_headers`/`response_headers` réels |
| **`browser_wait_for(ref=...)`** | N/A | Documenté mais **jamais câblé**, même pas accepté par la fonction interne | Corrigé (V22), testé 4 scénarios |
| **Sécurité transport** | HTTP/SSE local (CVE-2025-9611 : DNS rebinding) | stdio only | stdio only, structurellement immunisé (analysé V3) |
| **Détection erreurs JS non catchées** | Partielle | ✗ | Livré (`Runtime.exceptionThrown`), messages nettoyés (V15) |
| **Détection WCAG basique** | ✗ | ✗ | Livré (2 règles : nom manquant, imbrication) |
| **Bug des 2 onglets Chrome au démarrage** | N/A (n'a pas ce problème) | Présent (non détecté) | Corrigé, vérifié physiquement |
| **Bruit logs stderr (`Network.*ExtraInfo`)** | N/A | Présent (documenté "mineur") | Corrigé : 6945→3 lignes |
| **Gap couverture CDP corrigés** | N/A | 1 (`getFullAXTree`/`ignoredReasons`) | 2 (+ `Network.*ExtraInfo`) |
| **Flags de lancement Chrome** | Défauts Playwright (proche navigateur normal) | 25 (défauts `chromiumoxide`) | 32 (+7, Chromium ultra-configuré, voir V10) |
| **Vrai mode headless CDP** | Oui | Ignoré silencieusement malgré le paramètre exposé (bug non détecté avant V19) | Corrigé (V19), vérifié (`--headless` réellement présent dans la commandline Chrome) |
| **Classe de bug "paramètre documenté mais jamais lu"** | N/A | 5 occurrences non détectées (recipes, tabs, mode/headless, wait_for ref, + le cas doc-only des headers réseau) | Toutes corrigées et auditées exhaustivement (V20/V23) — classe fermée |

*\* `launch` à ~0 ms grâce au pool pré-chauffé : le Chrome du projet courant est déjà en cours de démarrage pendant le handshake MCP, donc le premier `browser_launch` le récupère quasi instantanément au lieu d'en attendre un nouveau. Ce n'est pas magique — c'est le même travail fait plus tôt, en parallèle du reste. Sur un projet différent du cwd, ou après expiration du TTL de 20s, `launch` retombe sur le chemin normal (~1,1s).*

**Ce que ce tableau ne montre pas, et qui compte tout autant** : à chaque version, au moins une piste a été **testée puis délibérément abandonnée** plutôt que livrée à moitié fonctionnelle (pool pré-chauffé sans garde-fou en V2, `MutationObserver` bloquant en V2, règle WCAG "image sans alt" en V3, `VecDeque` en V24 jugé inutile après mesure). Ce report continu de features par prudence, documenté honnêtement à chaque fois, est aussi une donnée de fiabilité — pas seulement les lignes qui avancent. Le détail complet des 26 vagues de développement (bugs trouvés, méthodologie de mesure, audits qui n'ont rien trouvé) reste documenté plus bas dans ce fichier.

## Résumé

| Métrique | Playwright MCP | Spectra | Verdict |
|---|---|---|---|
| Démarrage navigateur (`launch`) | 103 ms | 1 128 ms | Playwright plus rapide (voir limitations) |
| Navigation + attente de montage | 2 960 ms (mode `networkidle` strict) | 4 675 ms (détection DOM-settled adaptative) | Playwright plus rapide sur cette mesure précise (voir analyse) |
| Snapshot d'accessibilité, à froid | 79 ms | 52–96 ms | Comparable |
| Snapshot d'accessibilité, à chaud | 26 ms | 15–70 ms | Comparable |
| Taille du snapshot | 8 666 octets / 334 lignes | 10 495 octets / 349 lignes (232 refs) | Comparable (Spectra élague déjà les rôles `generic`/`none`/`presentation`) |
| Clic sur un élément | 96 ms | 96–200 ms | Comparable |
| RAM du process hôte (hors Chrome) | ~130 Mo (process Node.js) | ~27 Mo (binaire Rust natif) | **Spectra ~4,8x plus léger** |
| RAM Chrome headless (identique des deux côtés) | ~1,5 Go / 17 sous-process | ~1,5 Go / 17 sous-process | Égalité — Chrome est Chrome |

**Le vrai différenciateur n'est pas la latence brute** (les deux sont dans le même ordre de grandeur, chacun avec ses forces) **mais l'empreinte du process hôte, l'extensibilité par projet, et l'architecture qui élimine une classe entière de bugs.**

## Analyse honnête des latences

### Pourquoi le `launch` de Spectra est plus lent (1 128 ms vs 103 ms)

Playwright réutilise un navigateur "warm" géré par son propre cache d'exécutables et des heuristiques de démarrage optimisées depuis des années. Spectra en V1 lance un Chrome frais avec un profil `user-data-dir` neuf à chaque premier appel sur un projet — c'est un vrai point d'optimisation identifié pour la V2 (garder un pool de process Chrome pré-chauffés). **Ce n'est pas caché : c'est la limitation la plus visible de cette V1.**

### Pourquoi le `navigate` de Spectra est plus lent sur cette mesure précise

Un test isolé (mesure directe au niveau CDP, sans passer par le protocole MCP) a montré que sur l'app AstroQuest en mode dev, `document.body.innerText.length` ne se stabilise qu'à environ **2,6 secondes** après le `goto()` — c'est le vrai temps que React met à hydrater complètement la page derrière Vite HMR. Spectra utilise une détection adaptative (`wait_dom_settled`) qui poll le contenu du DOM jusqu'à stabilité plutôt qu'un délai fixe, avec un timeout de sécurité à 4 secondes. Playwright, avec son mode `networkidle`, attend un signal différent (absence de requête réseau active) qui se déclenche plus tôt sur cette app précise mais qui, sur d'autres sites avec du contenu chargé après le silence réseau (lazy-loading tardif), donnerait un DOM incomplet — exactement le piège dans lequel Spectra est tombé avant correction (voir "Bugs trouvés et corrigés" plus bas).

**En clair : c'est un compromis de robustesse, pas un défaut de conception.** La V2 réduira ce coût via un diff incrémental de l'arbre d'accessibilité (voir Roadmap).

### Où Spectra est déjà meilleur

- **Densité de snapshot** : à contenu comparable (334 vs 349 lignes, différence due à la profondeur de capture, pas au bruit), Spectra élague déjà les rôles purement présentationnels (`generic`, `none`, `presentation`, `InlineTextBox`) — un futur passage d'élagage des attributs par défaut (`[enabled]`, `[visible]`, `[focusable]` implicites) est identifié comme quick win V2 pour un gain supplémentaire de 20-30% de tokens.
- **RAM du process hôte** : 27 Mo contre 130 Mo. Sur une machine qui tourne déjà Claude Code, plusieurs projets, et potentiellement plusieurs sessions Spectra en parallèle, ce facteur 4,8x compte réellement — c'est le genre d'écart qui décide si une machine de développement reste fluide ou commence à ramer.

## Catalogue d'outils

Spectra expose **17 tools MCP natifs en V1** (21 aujourd'hui — voir le catalogue à jour dans le [README](../README.md#catalogue-des-tools)), conçus dès le départ pour un agent IA consommateur (refs courtes `[eN]`, retours structurés, dédup des logs/requêtes) :

| Tool | Rôle |
|---|---|
| `browser_launch` | Démarre ou rattache Chrome pour le projet courant |
| `browser_snapshot` | Capture l'arbre d'accessibilité condensé avec refs `[eN]` |
| `browser_navigate` | Navigue vers une URL, attend le montage applicatif |
| `browser_click` | Clique sur un élément référencé |
| `browser_type` | Saisit du texte, avec soumission optionnelle |
| `browser_fill_form` | Remplit plusieurs champs en un seul appel |
| `browser_select_option` | Sélectionne une option de `<select>` |
| `browser_hover` | Survole un élément (menus, tooltips) |
| `browser_screenshot` | Capture image (page entière, viewport, ou élément) |
| `browser_wait_for` | Attend un texte, une ref, ou un délai |
| `browser_console_messages` | Messages console dédupliqués avec compteur d'occurrences |
| `browser_network_requests` | Requêtes réseau groupées par domaine/statut |
| `browser_network_request_detail` | Détail complet d'une requête précise |
| `browser_evaluate` | Exécute du JavaScript dans le contexte de la page |
| `browser_tabs` | Liste/ouvre/ferme/sélectionne un onglet |
| `browser_file_upload` | Upload de fichiers vers un `<input type=file>` |
| `browser_run_recipe` | Exécute une recipe `.mjs` du projet — extensibilité sans recompilation |
| `browser_sessions` | Liste, ferme une, ou ferme **toutes** les sessions Chrome d'un coup |

### Extensibilité par projet — un différenciateur structurel

Playwright MCP n'a pas d'équivalent direct : Spectra découvre automatiquement un dossier `.spectra/recipes/*.mjs` par projet, exécuté par le Node.js déjà installé sur la machine, en process ponctuel (pas de runtime Node permanent). Exemple concret :

```js
// <projet>/.spectra/recipes/login_admin.mjs
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

Appelée depuis Claude Code via `browser_run_recipe(project="mon-erp", recipe="login_admin", args={email: "..."})` — un seul serveur central, zéro recompilation, extensible par n'importe qui sait écrire du JavaScript.

### Gestion multi-session — piloter plusieurs navigateurs en parallèle

Un vrai problème rencontré en cours de développement : sans limite, chaque test spawn un nouveau Chrome, et ils s'accumulent (jusqu'à 44 process observés en une session de tests intensifs). Corrigé avec :

- **`SPECTRA_MAX_SESSIONS`** (défaut 6) : plafond configurable du nombre de sessions Chrome simultanées sur ce serveur, tous projets confondus.
- **Réutilisation automatique** : un second `browser_launch` sur le même projet ne spawn jamais un second Chrome.
- **`browser_sessions(action="close_all")`** : ferme toutes les sessions actives en un appel — testé et validé.
- **`scripts/kill-all-spectra.ps1`** : script de nettoyage d'urgence, cible précisément les process Chrome lancés par Spectra (jamais le Chrome personnel de l'utilisateur), à lancer à tout moment si des sessions traînent.

La limite n'est volontairement pas fixée à 1 : le but est aussi de permettre plusieurs navigateurs en parallèle pour simuler du **multi-utilisateur** — par exemple tester une édition collaborative de documents à plusieurs sur une même app.

## Bugs trouvés et corrigés pendant cette session

Deux vrais bugs ont été identifiés et corrigés en testant contre une application de production réelle plutôt qu'un exemple jouet — signe que la V1 a été mise à l'épreuve, pas juste compilée :

**1. Gap de couverture de `chromiumoxide_cdp 0.7.0`** : le protocole CDP définit une catégorie de raisons pour lesquelles Chrome ignore un nœud d'accessibilité (`ignoredReasons`), incluant des valeurs comme `"uninteresting"` ou `"notRendered"`. L'enum Rust généré par la bibliothèque CDP ne couvrait pas cette catégorie entière, ce qui faisait échouer **tout** `Accessibility.getFullAXTree` dès qu'une page contenait un seul nœud avec cette raison — soit quasiment toutes les pages réelles. Contourné en réimplémentant l'appel CDP en désérialisation JSON brute (`raw_ax.rs`), qui lit uniquement les champs utiles sans jamais tenter de parser le champ cassé.

**2. Détection de montage applicatif trop rigide** : la première version attendait un délai fixe après navigation. Un test isolé a montré qu'une SPA React/Vite en cold-start peut mettre jusqu'à 2,6 secondes à finir son hydratation — un délai fixe trop court capturait un DOM quasi-vide (1 ligne au lieu de 349), un délai fixe trop long pénalisait les pages qui montent vite. Corrigé par une détection adaptative qui poll la stabilité du contenu DOM jusqu'à convergence, bornée par un timeout de sécurité.

## Limitations connues (V1)

- **Démarrage initial plus lent que Playwright** (~1,1s vs ~103ms) : pas de pool de process Chrome pré-chauffés.
- **`wait_until=networkidle`** garde une limitation sur les apps avec polling réseau continu (websocket HMR de dev server, heartbeat applicatif) — le mode par défaut (détection de stabilité DOM) est recommandé en usage normal.
- **`console_messages`/`network_requests`** sont bornés à 500 entrées en mémoire (ring buffer FIFO) — suffisant pour une session de test, pas pour un monitoring longue durée.

## Roadmap V2 (issue d'une revue critique multi-agents)

Identifiée par une session dédiée de critique architecturale, priorisée par impact/effort :

- **Diff incrémental de l'arbre d'accessibilité** : abandonner le `getFullAXTree` systématique au profit d'un arbre miroir en mémoire + abonnement aux événements de mutation DOM. Un clic qui ne change qu'une modale coûterait ~80 tokens au lieu de plusieurs milliers — c'est le changement qui porte la majorité du gain de vitesse visé sur les pages lourdes.
- **`browser_report`** : compilation automatique d'une session (actions, erreurs, captures, requêtes en échec) en un rapport structuré, dense pour l'agent ou exportable en Markdown/HTML pour un humain.
- **`browser_act_sequence`** : exécution d'une séquence d'actions entière côté Rust sans repasser par un tour de raisonnement LLM entre chaque étape, avec conditions d'arrêt sur imprévu (dialog, erreur console).
- **Élagage des attributs par défaut** du snapshot (`[enabled]`, `[visible]`, `[focusable]` implicites) — gain estimé 20-30% de tokens immédiat.
- **Compression des collections répétitives** (tableaux/listes de plus de 8-10 éléments similaires) en motif + premier/dernier — ratio de compression estimé ~45:1 sur un tableau ERP typique.

## V2 — améliorations livrées

Cette section documente ce qui a réellement été implémenté, mesuré et livré suite à la roadmap ci-dessus — avec la même honnêteté que le reste de ce document : ce qui a marché, ce qui a été tenté puis délibérément abandonné, et pourquoi.

### Livré et fonctionnel

- **Compression des tableaux/listes répétitives** : les siblings consécutifs de même rôle au-delà de 8 éléments sont désormais compressés en un résumé (`[N × role, motif répété]`) + premier et dernier élément détaillés (avec leurs refs), plutôt que d'être tous déroulés. Un tableau de 50 lignes ne produit plus 300 nœuds de texte mais un résumé + 2 lignes exploitables. Compromis assumé : les éléments du milieu n'ont pas de ref individuelle dans ce mode — un agent qui doit cliquer sur la ligne 25 précise devra d'abord réduire la vue (filtrer, chercher) plutôt que de viser directement sa ref.
- **`browser_report`** : un ledger de session (navigations, actions réussies/échouées, erreurs) s'alimente passivement à chaque appel d'outil, sans changer leur comportement. `browser_report` compile ce journal en un résumé structuré (compteurs + liste d'événements timestampés) — un agent n'a plus besoin de se souvenir de tout l'historique de la conversation pour savoir ce qui s'est passé dans une session de test.
- **`browser_act_sequence`** : exécute une liste d'actions (`click`, `type`, `select_option`) séquentiellement côté serveur en un seul appel MCP, avec gestion d'erreur par étape (`abort_sequence` ou `skip_and_continue`) et un rapport détaillé (étapes complétées + point de blocage). Réduit un formulaire de N champs à un seul aller-retour MCP dans le cas nominal.
- **`browser_snapshot(diff_only=true)`** : compare le nouveau texte de snapshot au précédent (stocké par page) et retourne un résumé compact ("N lignes inchangées, X ajoutées, Y retirées") au lieu de l'arbre complet, quand rien ou peu de choses ont changé depuis le dernier appel sur la même page.
- **Pipeline `mousePressed`/`mouseReleased`** : les deux événements d'un clic sont désormais émis en parallèle (`futures::join!`) plutôt que séquentiellement, réduisant le nombre d'allers-retours attendus par clic (le `mouseMoved` initial reste séquentiel, nécessaire pour les listeners `mouseenter`/hover).

### Tenté puis abandonné par prudence

Deux pistes de la roadmap initiale ont été implémentées, testées en conditions réelles contre l'app AstroQuest, puis **retirées délibérément** après avoir révélé des risques jugés inacceptables :

- **Pool de Chrome pré-chauffé au démarrage du serveur** : l'idée était de spawn un Chrome en tâche de fond dès `SpectraTools::new()`, pendant le handshake MCP, pour rendre le premier `browser_launch` quasi-instantané. En pratique, dès que le premier projet demandé différait du cwd du serveur (cas fréquent dans les tests), ce Chrome pré-chauffé n'était **jamais consommé** et restait ouvert indéfiniment tant que le serveur tournait — exactement le type d'accumulation de process Chrome que ce projet cherche à éviter. Le gain de latence, lui, n'était même pas prouvé en usage réel (le test synthétique qui l'aurait validé enchaînait les appels sans le délai naturel qu'un usage humain introduirait). Verdict : risque certain contre gain hypothétique → retiré.
- **Détection de fin de montage DOM via `MutationObserver` + `Runtime.evaluate(awaitPromise: true)`** : en théorie plus efficace qu'un polling depuis le serveur (un seul aller-retour CDP au lieu de plusieurs). En test réel, cet appel s'est bloqué **plus de deux minutes** sans jamais retourner, malgré un timeout CDP explicite de 4,5 secondes configuré côté serveur — signe d'une interaction mal comprise entre `chromiumoxide` et ce pattern de promesse. Remplacé par un polling côté Rust classique (`document.body.innerText.length`, comparaison de stabilité sur deux mesures), avec un `tokio::time::timeout` strict sur *chaque* appel individuel — plus verbeux niveau protocole, mais dont le pire cas est mathématiquement borné.

### Nouveaux chiffres mesurés (build release V2, thin LTO)

| Métrique | V1 | V2 | Playwright |
|---|---|---|---|
| `launch` | 1 128 ms | 1 092 ms | 103 ms |
| `navigate` (mode par défaut) | 4 675 ms | 3 612 ms | 2 960 ms (mode `networkidle` strict) |
| `snapshot` à froid | 29-96 ms | 32 ms | 79 ms |
| Taille snapshot (même page) | 268-349 lignes | 114 lignes* | 334 lignes |

*\* Le nombre de lignes plus bas en V2 reflète la compression des tableaux/listes (nouvelle en V2), pas une perte de contenu — le texte capturé reste complet, juste densifié.*

### Limitation additionnelle découverte pendant cette session

Un second gap de couverture CDP a été identifié dans `chromiumoxide_cdp 0.7.0` (même famille de bug que celui corrigé en V1 sur `Accessibility.getFullAXTree`) : les événements `Network.requestWillBeSentExtraInfo` et `Network.responseReceivedExtraInfo` échouent systématiquement à se désérialiser (`data did not match any variant of untagged enum Message`). Contrairement au bug V1, celui-ci **n'empêche pas le fonctionnement** — ce sont des événements informatifs annexes (détail des cookies/headers bruts), pas un chemin critique. Il génère seulement du bruit dans les logs stderr en mode debug. Documenté comme limitation connue mineure plutôt que corrigé dans l'immédiat, faute de vrai impact fonctionnel.

## Ce qui reste pour la V3 (planifié initialement)

- **Vrai diff structurel de l'arbre d'accessibilité** : la V2 livre un diff textuel naïf (comparaison ligne-à-ligne). L'architecture cible reste un arbre miroir en mémoire + abonnement aux événements CDP de mutation, avec des refs qui restent stables tant que le nœud sous-jacent n'a pas changé — le vrai gain de performance sur les pages lourdes.
- **Pool de navigateurs pré-chauffés avec TTL** : reprendre l'idée abandonnée en V2, mais avec une fermeture automatique du warm-up non consommé après un délai raisonnable, pour éliminer le risque de fuite tout en gardant le gain de latence.
- **Détection de montage DOM événementielle robuste** : retenter une approche événementielle (peut-être via `Page.lifecycleEvent` plutôt qu'un `MutationObserver` custom) avec des garde-fous plus stricts, pour remplacer le polling actuel sans revivre le blocage rencontré.
- **Correction du gap `Network.*ExtraInfo`** : suivre le même pattern que `raw_ax.rs` (désérialisation JSON brute) si ces événements deviennent nécessaires à une future fonctionnalité (ex: inspection fine des cookies bloqués).

## V3 — améliorations livrées

Suite à une session de recherche multi-agents dédiée (angles : developer experience, capacités de détection, intégrations écosystème, benchmark concurrentiel), voici ce qui a été implémenté, testé en conditions réelles, et livré.

### Sécurité — analyse et verdict

L'analyse concurrentielle a soulevé un risque connu sur d'autres serveurs MCP navigateur : **CVE-2025-9611**, une vulnérabilité de type DNS rebinding sur Playwright MCP, exploitable quand le serveur expose un transport HTTP/SSE local sans validation stricte du header `Host`. Vérification faite : Spectra n'active que la feature `transport-io` (stdio) de `rmcp` — aucun transport HTTP/SSE n'est compilé dans le binaire. Le vecteur d'attaque ne s'applique donc **structurellement pas** : il n'y a aucun socket réseau local à cibler. Documenté dans le README comme argument de sécurité positif plutôt que comme simple note défensive.

### Messages d'erreur actionnables

Avant : une erreur de démarrage de Chrome remontait telle quelle depuis `chromiumoxide`, souvent un message technique opaque. Maintenant :
- Détection préalable de la présence de Chrome (emplacements standards Windows + variable `SPECTRA_CHROME_PATH`) avant même de tenter le spawn, avec un message pointant vers la solution (installer Chrome, ou définir la variable).
- Erreurs de spawn enrichies avec les causes probables (port déjà utilisé, profil verrouillé par une instance zombie — avec pointeur vers `scripts/kill-all-spectra.ps1`, permissions insuffisantes).

### Détection d'exceptions JavaScript non catchées

`Runtime.exceptionThrown` (CDP) est maintenant écouté et pousse les erreurs JS non catchées et rejections de promesses non gérées dans le même buffer console que les logs habituels, au niveau `error`, avec la localisation (fichier:ligne:colonne) quand disponible. **Testé en conditions réelles** : une erreur JS levée volontairement via `browser_evaluate` apparaît correctement dans `browser_console_messages(level="error")` avec sa stack trace, sans latence ni duplication d'abonnement CDP.

### `check_a11y` sur `browser_snapshot` — détection WCAG basique

Nouveau paramètre optionnel qui ajoute un champ `a11y_issues` au retour du snapshot, en post-traitement pur de l'arbre d'accessibilité déjà collecté (aucun appel CDP supplémentaire, donc aucun risque de latence). Deux règles livrées :
- **`missing-accessible-name`** : élément interactif (bouton, lien, champ de saisie...) sans nom accessible — inutilisable au clavier/lecteur d'écran.
- **`nested-interactive`** : rôle interactif imbriqué dans un autre rôle interactif (ex: un bouton dans un lien) — structure ARIA ambiguë.

**Une troisième règle abandonnée, et pourquoi** : une règle "image sans texte alternatif" a été implémentée puis testée contre l'app réelle. Premier essai : filtrer les images dont le parent direct porte déjà un nom (icône décorative dans un bouton nommé) — insuffisant, un faux-positif massif (70+ occurrences identiques) persistait. Diagnostic approfondi (spike Rust isolé inspectant la structure réelle de l'arbre) : le motif dominant était `button[nommé] > div[role=none] > image[sans nom]`, un wrapper de présentation intermédiaire que le premier filtre ne traversait pas. Deuxième essai : remonter récursivement les ancêtres au-delà des rôles présentationnels purs — toujours insuffisant, le même volume de faux-positifs persistait, signe d'un motif structurel plus profond non identifié dans le temps disponible. Plutôt que de livrer une règle dont le signal est noyé dans le bruit, elle a été **retirée** du code. Ce n'est pas de la volonté de trop en faire à tout prix : une fonctionnalité à moitié fiable est pire qu'une fonctionnalité absente, car elle érode la confiance dans tout le reste des résultats retournés.

### `browser_performance_metrics` — Core Web Vitals (LCP, CLS, FCP, TTFB)

Nouveau tool qui mesure les quatre métriques de performance perçue les plus regardées (Largest Contentful Paint, Cumulative Layout Shift, First Contentful Paint, Time To First Byte), sans dépendance externe (pas de Lighthouse, pas de web-vitals.js) — uniquement les APIs natives du navigateur exposées via CDP.

**Un piège non trivial trouvé et corrigé en diagnostic isolé** : la première approche naïve (`performance.getEntriesByType('largest-contentful-paint')` interrogé après coup, une fois la page chargée) retournait systématiquement un tableau vide sur l'app réelle — `lcp_raw_count: 0` à chaque essai, alors que Chrome liste bien `largest-contentful-paint` parmi ses `supportedEntryTypes`. Cause : sans un `PerformanceObserver` actif *avant* que l'entrée ne soit générée, Chrome ne la conserve pas dans un buffer interrogeable a posteriori pour ce type d'entrée précis. Solution : poser l'observer via `Page.addScriptToEvaluateOnNewDocument` (`evaluate_on_new_document` côté chromiumoxide), donc avant même le premier octet de la page suivante — le même point d'installation que les observers console/réseau existants (`observers::ensure_observing`).

**Second piège, plus sournois** : en ajoutant un second `PerformanceObserver` sur `layout-shift` avec `buffered: true` (pour, en théorie, rattraper l'historique), `Page.goto()` s'est mis à **bloquer indéfiniment** (timeout de 15s systématique en test isolé, reproductible à chaque essai) — un symptôme qui, sans le diagnostic étape-par-étape (impression + timeout strict autour de chaque appel CDP individuel), aurait pu être confondu avec un simple ralentissement réseau. Isolé en testant chaque observer séparément : `layout-shift` avec `buffered: true` seul suffit à reproduire le blocage ; le même observer avec `buffered: false` fonctionne sans latence additionnelle. `buffered: true` n'apportait de toute façon aucun bénéfice réel ici : le script s'exécute avant tout rendu, donc rien n'a encore pu être émis au moment de l'appel `observe()`. Fix appliqué : seul l'observer LCP garde `buffered: true` (nécessaire, l'ordre entre le chargement du document et l'exécution du script n'est pas garanti par CDP) ; l'observer CLS tourne en `buffered: false`.

**Mesuré en conditions réelles contre AstroQuest** (localhost:8080, après navigation + 3s de stabilisation) : `lcp_ms: 3932`, `fcp_ms: 440`, `ttfb_ms: 44`, `cls: 0` — résultats stables sur deux appels consécutifs sans duplication d'observer ni dérive d'état.

## V4 — améliorations livrées

### Pool de Chrome pré-chauffé — repris avec un TTL de fermeture automatique

La V2 avait tenté puis abandonné cette idée (voir "Tenté puis abandonné par prudence" plus haut) : un Chrome pré-chauffé dès `SpectraTools::new()` pouvait rester ouvert indéfiniment si le premier projet demandé différait du cwd du serveur. Reprise ici avec un garde-fou explicite : le warm-up expire après **20 secondes** s'il n'a pas été consommé par un `browser_launch`/`browser_navigate` qui correspond exactement à sa clé de projet (le cwd du serveur). Passé ce délai, la dernière référence au Chrome pré-chauffé est explicitement abandonnée, ce qui déclenche le même `Drop` (`taskkill /T /F /PID`) que n'importe quelle session fermée normalement.

**Trois scénarios testés en conditions réelles, chacun avec vérification du nombre de process Chrome physiques (pas seulement de l'état en mémoire) :**

1. **Consommation** : `browser_launch` (sans projet explicite) appelé ~2s après le démarrage du serveur récupère bien le Chrome pré-chauffé — même port, `attached_existing: false` (ce n'est pas un attach, c'est la même instance spawn qui devient simplement "propriétaire" de la session), et surtout **aucun second Chrome** n'apparaît dans `tasklist`.
2. **Expiration** : warm-up jamais consommé, contrôlé 25s après le démarrage — `browser_sessions(list)` retourne `active_sessions: []` et `tasklist` confirme **zéro process Chrome restant**, preuve que la fermeture automatique agit sur le vrai process, pas juste sur une structure en mémoire.
3. **Non-interférence** : un `browser_launch` explicite sur un projet différent du cwd pendant la fenêtre TTL spawn sa propre session sans toucher au warm-up en attente — les deux coexistent sur des ports distincts jusqu'à ce que chacun suive son propre sort (consommation ou expiration).

`browser_sessions(action="list")` expose désormais le warm-up en attente (`warm_up_pending: true`) par transparence, et `close_all` le ferme explicitement en plus des sessions actives — un `close_all` ne doit jamais laisser un Chrome de côté simplement parce qu'il n'a pas encore été "officiellement" adopté par un tool.

**Compromis assumé** : pendant la fenêtre TTL (20s), si `SPECTRA_MAX_SESSIONS` sessions différentes du cwd sont ouvertes en parallèle, le nombre réel de process Chrome peut dépasser temporairement la limite configurée de 1 (les N sessions + le warm-up encore en attente). C'est borné dans le temps et en nombre — un dépassement de +1 pendant 20s maximum — jugé acceptable face au gain de latence sur le cas d'usage le plus fréquent (premier appel sur le projet courant).

## V5 — améliorations livrées

### Diff structurel par `backend_id`, en remplacement du diff textuel naïf

Le diff textuel ligne-à-ligne des V2/V3 (`diff_lines`, comparaison d'ensembles de lignes) avait un défaut de conception non détecté jusqu'ici : les refs `[eN]` sont un simple index de parcours recalculé à chaque capture, donc un nœud ajouté ou retiré n'importe où dans l'arbre décale toutes les refs qui le suivent. Un test isolé (page HTML minimale : un bouton "Toggle" qui retire un `<div>` placé avant trois autres boutons) a révélé que ce décalage pouvait, dans le pire cas, faire disparaître complètement un vrai changement du texte de diff si les lignes restantes se recombinaient par coïncidence — un faux négatif silencieux, plus grave qu'un simple bruit.

Remplacé par un diff structurel qui compare chaque nœud par son `backendDOMNodeId` (stable tant que Chrome ne détruit/recrée pas l'élément DOM sous-jacent, indépendant de sa position dans l'ordre de parcours) : présent avant et absent après = retiré ; absent avant et présent après = ajouté ; présent des deux côtés avec un rôle ou un nom différent = modifié ; identique des deux côtés = inchangé, même si sa ref `[eN]` a changé entre les deux captures.

**Retesté sur le même cas qui avait révélé le problème** : `browser_click(diff_only=true)` sur le bouton "Toggle" retourne désormais exactement `1 retiré: [e2] StaticText "Early item"`, sans aucun bruit sur les 4 autres éléments dont la ref a pourtant changé. **Testé aussi contre AstroQuest réel** : l'ouverture du launcher (`Ctrl+K`) retourne `19 ajoutés, 0 retirés, 3 modifiés` (la modale `dialog "Spotlight AstroQuest"` et son contenu), sur un total de 103 nœuds inchangés qui ne polluent pas la sortie.

**Limite honnête découverte pendant les tests** : un `<span>` dont le `textContent` est modifié par JavaScript n'est pas rapporté comme "modifié" mais comme un couple ajout+retrait (l'ancien `StaticText` disparaît, un nouveau apparaît avec un `backend_id` différent). Cause probable : Chrome semble recréer le nœud d'accessibilité du texte plutôt que de muter en place celui existant, contrairement à l'élément conteneur lui-même. Le résultat reste correct et lisible (le changement est bien signalé), juste pas classé dans la catégorie idéale. Corriger ce cas précis nécessiterait de comparer aussi par position/contexte parent en plus du `backend_id`, une complexité jugée non justifiée pour un gain marginal — documenté ici plutôt que caché.

`diff_only` est désormais aussi disponible sur `browser_click`, `browser_type`, `browser_select_option` et `browser_hover` (auparavant réservé à `browser_snapshot` seul) : une action suivie d'un besoin de diff ne nécessite plus un second appel MCP séparé.

### Fix : deux onglets Chrome ouverts au lieu d'un seul

Signalé en usage réel (capture d'écran d'une session avec tête : un onglet "New Tab" vide + un second onglet avec la page réellement pilotée). Cause identifiée en lisant le code de `chromiumoxide` : Chrome ouvre toujours son propre onglet natif au démarrage, indépendamment de notre code, mais l'événement CDP qui le rend visible via `Browser::pages()` est traité de façon asynchrone par le handler — au tout premier appel juste après `Browser::launch()`, cette liste peut encore être vide, ce qui faisait conclure à tort qu'aucun onglet n'existait et déclenchait la création d'un second (`new_page("about:blank")`).

Corrigé par un retry court (jusqu'à 10 tentatives, 50ms d'intervalle, uniquement quand ce process a lui-même spawné Chrome) avant de conclure qu'il faut créer un nouvel onglet. **Vérifié physiquement** (pas seulement via le registre interne du serveur, mais en interrogeant directement `http://127.0.0.1:<port>/json/list` sur le vrai Chrome) dans les deux modes : une seule page réelle après `browser_navigate`, en headless comme avec tête.

## V6 — améliorations livrées

### `diff_only` étendu à `browser_fill_form` et `browser_act_sequence`

Après l'avoir déjà étendu à `browser_click`/`type`/`hover`/`select_option`, les deux derniers tools qui retournaient encore systématiquement l'arbre complet après une action l'ont aussi reçu :

- **`browser_fill_form(diff_only=true)`** : utile en particulier pour un formulaire avec validation live (un message d'erreur ou de confirmation qui apparaît pendant la saisie) — testé sur un formulaire à deux champs avec un listener JS qui affiche un texte de validation, le diff retourné ne montre que les deux changements réels (`"Regis"` saisi + `"validation ok"` apparu), sans bruit sur le reste du formulaire.
- **`browser_act_sequence(diff_only=true)`** : contrairement aux autres tools, `browser_act_sequence` ne retournait auparavant **aucun snapshot du tout** (juste un statut par étape). Avec `diff_only=true`, un diff structurel est désormais capturé une seule fois après la dernière étape exécutée — jamais par étape individuelle, ce qui annulerait le bénéfice de regrouper plusieurs actions en un seul appel MCP. Testé en conditions réelles contre AstroQuest (séquence d'un seul clic ouvrant le launcher) : résultat identique à celui obtenu via `browser_click(diff_only=true)` directement.

## V7 — améliorations livrées

### Bruit massif éliminé dans les logs stderr (`Network.*ExtraInfo`)

La limitation documentée en V2 ("génère seulement du bruit dans les logs stderr en mode debug, sans impact fonctionnel") s'est révélée bien plus sévère en investigation réelle : sur une session de navigation normale, **6929 lignes sur 6945 mesurées (99.8% du volume total de logs) et au niveau `ERROR`**, pas seulement en mode debug — donc visible même avec le niveau par défaut du serveur.

Cause précisément identifiée en capturant le message CDP brut fautif : `Network.responseReceivedExtraInfo` contient un champ `resourceIPAddressSpace` avec la valeur `"Loopback"`, une variante d'enum ajoutée par une version récente de Chrome mais absente de l'enum correspondant généré par `chromiumoxide_cdp 0.7.0` — exactement le même type de gap de couverture CDP que celui déjà rencontré et contourné pour `Accessibility.getFullAXTree` en V1 (`raw_ax.rs`), mais cette fois le point d'échec est le **parseur générique de la connexion WebSocket** (`chromiumoxide::conn`), en amont de tout listener applicatif — donc un listener dédié en désérialisation tolérante n'aurait rien changé ici.

Vérification faite en lisant le code de `chromiumoxide::handler` avant de corriger quoi que ce soit : cette erreur ne casse rien, `Handler::poll_next` la propage comme un item `Err` du flux d'événements, que notre boucle `while handler.next().await.is_some() {}` absorbe et continue sans interruption — seul le bruit de log est un problème réel, pas le comportement. Corrigé en filtrant les cibles `chromiumoxide::conn` et `chromiumoxide::handler` dans le filtre `tracing` par défaut du serveur (`chromiumoxide::conn=off,chromiumoxide::handler=off`), sans toucher à la bibliothèque elle-même.

**Mesuré avant/après sur la même session de test** (une navigation vers AstroQuest, en usage normal sans `RUST_LOG` positionné) : 6945 lignes de logs stderr avant correction, **3 lignes après** — les deux logs de démarrage légitimes plus un log de requête, sans aucune perte d'information utile (le filtre reste `info` par défaut sur tout le reste, et `RUST_LOG` explicite continue de tout primer si un développeur veut vraiment inspecter ce trafic bas niveau).

## Le cycle d'amélioration continue de Spectra

Ce document n'est pas un rapport figé après un sprint unique : sept vagues de développement (V1→V7) ont été livrées dans la même session de travail, chacune suivant la même discipline — implémenter, tester contre l'application réelle (jamais un exemple jouet), documenter honnêtement (y compris les abandons), rebuild, redéployer, puis chercher la lacune suivante plutôt que de s'arrêter sur un état "suffisant". C'est un principe de fonctionnement volontaire, pas un hasard de planning : un serveur qui pilote un navigateur touche à trop de couches instables (CDP, timing async, cycle de vie de process OS) pour qu'une seule passe de développement l'épuise.

### Ce que révèle le tableau ci-dessus sur les priorités à venir

Deux lignes du tableau restent identiques à Playwright ou pires depuis la V1, sans qu'aucune vague n'ait encore osé s'y attaquer :

- **`navigate` (~3,6 s vs 2 960 ms)** : toujours plus lent que Playwright sur cette mesure précise, et strictement inchangé depuis la V2. La roadmap V3 avait identifié une piste ("diff incrémental de l'arbre d'accessibilité" via abonnement aux événements de mutation CDP) qui n'a *jamais été retentée* après l'échec du `MutationObserver` en V2 — le diff structurel de la V5 a résolu la fiabilité du diff, pas la latence de la première capture après navigation. Il reste un vrai gain non exploité ici, actuellement laissé de côté par prudence plutôt que par manque d'idée.
- **Snapshot initial toujours à contenu comparable, jamais réellement plus léger** que Playwright en octets bruts (seule la V2 a réduit le nombre de *lignes* via la compression des tableaux, pas la densité par ligne). L'élagage des attributs par défaut (`[enabled]`, `[visible]`, `[focusable]` implicites), identifié comme "quick win V2" dans la toute première roadmap, n'a **jamais été livré** — six vagues plus tard, cette piste dort encore dans un paragraphe de 2026-07-04.

## V8 — améliorations livrées

Suite directe de la section précédente : les deux métriques identifiées comme stagnantes depuis la V1 (latence de `navigate`, densité du snapshot) ont été attaquées, plus un troisième axe demandé explicitement — le contrôle du coût en tokens par l'agent lui-même, via des niveaux de verbosité configurables.

### Latence de `navigate` réduite (poll interval 150ms → 80ms)

Un spike isolé (mesure fine, poll toutes les 50ms sans le biais de l'algorithme de production) a d'abord confirmé où va le temps sur AstroQuest : `goto()` seul prend ~1,4s (chargement réseau), puis le texte du DOM se stabilise **définitivement vers t≈3,3s** après le début de la navigation — cohérent avec les ~2,6s déjà documentés en V1. Le vrai levier trouvé n'était pas la stabilisation React elle-même (hors de contrôle de Spectra) mais la **queue artificielle** ajoutée par `wait_dom_settled` : avec un `poll_interval` de 150ms et 2 mesures stables consécutives requises, le serveur attend mécaniquement au moins un tour de poll complet après que le DOM soit déjà figé avant de pouvoir conclure.

Réduit à 80ms sans toucher au nombre de mesures stables requises (`stable_streak >= 2` reste le même garde-fou contre un DOM encore en mouvement — c'est la même protection, avec un grain plus fin). **Mesuré en conditions réelles via le protocole MCP** (pas juste le spike) : `navigate` passe de 3 612 ms (V2) à **3 380 ms**. Le gain est modeste par nature (borné par le temps de poll économisé, pas par la stabilisation React elle-même, qui reste le vrai goulot), mais réel et sans aucune perte de robustesse mesurée.

### Propriétés d'état exposées dans le snapshot (`[disabled]`, `[checked]`, `[expanded]`, ...)

`AXNode.properties` (le champ CDP qui porte l'état d'interaction d'un élément) n'était jusqu'ici jamais lu par `raw_ax.rs` — la roadmap V2 imaginait "élaguer des attributs par défaut déjà présents", mais en réalité ces attributs n'étaient tout simplement pas encore capturés du tout. Corrigé en lisant ce champ avec une **whitelist stricte** plutôt qu'une blacklist de bruit : un spike dédié (page HTML de test + AstroQuest réel) a montré que `focusable` et `invalid=false` sont présents sur quasiment tous les nœuds sans jamais être informatifs — les inclure aurait gonflé chaque ligne sans bénéfice, exactement le piège qui avait fait échouer la règle WCAG "image sans alt" en V3.

Sept propriétés à fort signal retenues : `disabled`, `required`, `readonly`, `checked` (+ `checked:mixed` pour l'état tri-state), `pressed`, `expanded`, `selected` — chacune n'apparaît que si sa valeur diffère de l'état neutre implicite (`[e4] checkbox "Unchecked"` reste sans flag, `[e5] checkbox "Checked" [checked]` en affiche un). **Testé sur une page HTML contrôlée** : les 4 combinaisons (bouton désactivé, checkbox cochée/décochée, champ requis) s'affichent correctement, et le diff structurel de la V5 détecte bien un changement `[expanded]` après un clic (`~ [e7] button "Menu" [expanded] (était [aucun])`), sans avoir eu à modifier la logique de diff — le nouveau champ `state_flags` participe simplement à la comparaison de modification déjà existante. **Vérifié sur AstroQuest réel** : zéro flag affiché sur la page d'accueil (aucune checkbox/champ requis/menu visibles sur cet écran) — confirmation qu'aucun bruit n'est introduit là où rien ne le justifie.

### Niveaux de verbosité configurables (`detail="compact"` / `"refs_only"`)

Demande explicite : laisser un agent contrôler son propre coût en tokens plutôt que de toujours recevoir l'arbre complet. Trois niveaux ajoutés à `browser_snapshot`, appliqués en post-traitement sur la structure déjà collectée pendant le rendu principal (donc **zéro appel CDP supplémentaire**, juste un filtrage) :

- **`full`** (défaut, comportement historique) : arbre complet avec la hiérarchie structurelle.
- **`compact`** : ne garde que les nœuds qui ont reçu une ref (actionnables ou nommés), retire les nœuds purement structurels (`main`, `banner`, `paragraph` sans nom) tout en gardant l'indentation d'origine pour la lisibilité.
- **`refs_only`** : une ligne par ref, sans indentation ni hiérarchie — le minimum vital pour cibler un clic après avoir déjà lu un snapshot complet une première fois.

**Mesuré sur AstroQuest réel** (même page, même état) :

| Niveau | Caractères | Lignes | Gain vs `full` |
|---|---|---|---|
| `full` | 3 513 | 114 | — |
| `compact` | 2 785 | 72 | −20,7 % |
| `refs_only` | 2 349 | 72 | −33,1 % |

Les 72 refs actionnables restent identiques dans les trois modes — le gain vient uniquement du texte structurel/indentation retiré, jamais d'une perte de capacité d'action. Sans effet sur `diff_only=true` (déjà minimal par construction) ni sur `check_a11y` (analyse la même donnée sous-jacente quel que soit le niveau choisi).

### Chiffres consolidés V8

| Métrique | V7 | V8 |
|---|---|---|
| `navigate` (mode par défaut) | 3 612 ms | **3 380 ms** |
| Snapshot `detail=full` (page repère) | 3 513 o / 114 lignes | idem (comportement par défaut inchangé) |
| Snapshot `detail=compact` | N/A (n'existait pas) | 2 785 o / 72 lignes |
| Snapshot `detail=refs_only` | N/A (n'existait pas) | 2 349 o / 72 lignes |
| Propriétés d'état exposées | 0 | 7 (`disabled`, `required`, `readonly`, `checked`, `pressed`, `expanded`, `selected`) |

## V9 — améliorations livrées

### Bug critique corrigé : `browser_run_recipe` échouait à 100% en usage réel

En testant activement l'extensibilité par projet (jamais vérifiée en conditions réelles depuis la V1, malgré sa présentation comme différenciateur structurel majeur face à Playwright dans le README et cette page), `browser_run_recipe` échouait **systématiquement** avec `Cannot find module '...\Spectra\runtime\recipe-runner.mjs'`. Le fichier n'existait nulle part : ni installé sur le disque, ni même présent dans le dépôt source. `recipe.rs` (côté Rust) était entièrement écrit et fonctionnel — spawn du process Node, sérialisation du payload, lecture du résultat — mais pointait vers un runtime qui n'avait jamais été créé.

**Corrigé en deux temps** :

1. **Écrit le runtime manquant** (`runtime/recipe-runner.mjs`) : un client CDP minimal en JavaScript pur, sans aucune dépendance npm (le `WebSocket` global natif de Node 22+ suffit), qui implémente le contrat `ctx.navigate`/`ctx.snapshot`/`ctx.findRef`/`ctx.click`/`ctx.type`/`ctx.waitFor` déjà documenté dans le README. Cohérent avec le principe déjà énoncé ("pas de runtime Node permanent, pas d'install à gérer par projet") : zéro `npm install` requis, le Node déjà présent sur la machine suffit.
2. **Auto-installation embarquée** : plutôt que de dépendre d'un script d'installation séparé (le vrai defect racine — rien ne copiait jamais ce fichier), le contenu du runtime est désormais inclus dans le binaire Rust lui-même via `include_str!` à la compilation, et écrit sur disque à la demande (silencieusement, au premier appel de `browser_run_recipe`) s'il est absent ou différent de la version embarquée. Le binaire est donc totalement autonome — aucune étape d'installation manuelle ne peut plus être oubliée.

**Diagnostic du bug qui a suivi, plus subtil** : une fois le runtime en place, une première recipe de test (ouvrir le launcher AstroQuest via `Ctrl+K`) échouait à détecter l'ouverture de la modale. Neuf hypothèses testées une à une avant de trouver la vraie cause :
- Le clic natif (`Input.dispatchMouseEvent`, identique au pattern Rust déjà validé) semblait ne rien déclencher.
- Un test de bas niveau (listener `document.addEventListener` en phase de capture) a confirmé que l'événement arrivait bien, `isTrusted: true`, aux bonnes coordonnées.
- Un test avec `waitFor({text: "Ecosystem"})` (texte déjà présent avant le clic) a confirmé que le clic **avait bien fonctionné** — la vraie modale s'ouvrait (`refCount` passant de 232 à 282 nœuds).
- Le vrai problème : le texte cherché (`"Spotlight"`, le nom accessible `aria-label` de la modale) n'apparaît jamais dans `document.body.innerText` — cette API ne capture que le texte visuellement rendu, pas les attributs d'accessibilité invisibles. `waitFor` cherchait la mauvaise source de vérité, pas un vrai défaut du clic ou du dispatch CDP.

**Testé bout en bout, recipe complète** : `navigate` → `findRef` (repère le bouton "Ouvrir le launcher" via regex sur son nom) → `click` → `waitFor({text: "Ctrl+K"})` (un texte réellement visible cette fois) → `{launcherOpened: true}`. Vérifié aussi que la suppression manuelle du fichier installé puis un nouvel appel de `browser_run_recipe` le réinstalle silencieusement sans aucune intervention, confirmant que l'auto-installation fonctionne dans les deux sens (premier appel et récupération après suppression accidentelle).

## V10 — améliorations livrées

### Chromium "ultra-configuré" plutôt qu'un navigateur maison

Proposition initiale de l'utilisateur : construire un navigateur propre, natif contrôle IA, plutôt que de piloter Chrome. Réponse honnête donnée avant d'agir : réécrire un moteur de rendu web (parsing HTML/CSS/JS, layout) est un projet de dizaines de millions de lignes de code même pour les équipes Chromium/Firefox — aucun nouvel entrant sérieux n'a réussi ce pari en quinze ans (même Servo de Mozilla, après des années d'investissement dédié, reste un projet de recherche partiel). L'intention réelle derrière la demande — un navigateur sans "features parasites", pensé pour un contrôle programmatique plutôt qu'un humain — reste atteignable sans ce risque : en configurant Chromium au maximum plutôt qu'en le remplaçant. Option retenue par l'utilisateur après explication.

Ajout de 7 flags de lancement supplémentaires (au-delà des 25 déjà appliqués par défaut par `chromiumoxide`) qui retirent des sous-systèmes dont un agent IA n'a jamais l'usage : `--disable-gpu`/`--disable-software-rasterizer` (rendu GPU inutile en headless pur, où il n'y a de toute façon aucun affichage physique), `--disable-notifications`, `--disable-domain-reliability`, `--no-pings`, `--disable-background-networking`, et `--disable-features=Translate,BackForwardCache,AcceptCHFrame,MediaRouter,OptimizationHints,PrivacySandboxSettings4` (fonctionnalités de navigation humaine sans équivalent dans un pilotage programmatique).

**Méthodologie de mesure** : plutôt que de se fier à un seul run (un premier test isolé avait montré un gain trompeur de -31,5%, vraisemblablement une coïncidence de charge système plutôt qu'un effet réel des flags), 3 runs par variante avec un toggle explicite (`SPECTRA_MINIMAL_CHROME=0` pour désactiver temporairement ces flags lors d'une comparaison), sur le même binaire, contre la même page AstroQuest :

| Variante | Run 1 | Run 2 | Run 3 | Moyenne |
|---|---|---|---|---|
| Baseline (flags actuels) | 1233 Mo | 1248 Mo | 1226 Mo | **1236 Mo** |
| Avec les 7 flags additionnels | 1101 Mo | 1269 Mo | 1177 Mo | **1182 Mo** |

**Gain réel mesuré : environ 4,3% de RAM en moins**, cohérent avec l'attente honnête ("Chrome reste Chrome" — le moteur de rendu, le process principal, les workers JavaScript sont strictement identiques ; seuls quelques sous-systèmes annexes disparaissent). La variance individuelle entre runs (jusqu'à 168 Mo d'écart dans la même variante) dépasse largement l'effet moyen mesuré — un signe de prudence à garder en tête avant de citer ce chiffre comme une garantie stricte plutôt qu'une tendance statistique. **Aucune régression fonctionnelle détectée** : single-tab, Core Web Vitals, et diff structurel testés à nouveau après l'ajout des flags, tous identiques à avant.

## V11 — améliorations livrées

### Bug corrigé : `browser_tabs` n'implémentait en fait que `"list"`

Suite au mandat de continuité, un audit ciblé des tools jamais exercés en conditions réelles cette session (`browser_tabs`, `browser_select_option`, `browser_screenshot`, `browser_wait_for`, `browser_console_messages`, `browser_network_requests`, `browser_evaluate`, `browser_file_upload`, `browser_report`) a révélé un second gap du même acabit que celui des recipes en V9 : `browser_tabs` acceptait bien un paramètre `action` (`"list"`/`"new"`/`"close"`/`"select"`, documenté et exposé dans le schéma MCP), mais le handler ne faisait jamais de `match` dessus — il retournait systématiquement la liste des pages existantes, quelle que soit l'action demandée. `action="new"` ne créait donc jamais de second onglet, `"close"` ne fermait rien, `"select"` ne changeait jamais l'onglet actif.

**Corrigé** en ajoutant trois méthodes à `BrowserSession` (`new_page`, `close_page`, `select_page`) qui manquaient totalement — jusqu'ici seule `ensure_active_page` (interne, pour le cas "aucune page encore ouverte") existait. `browser_tabs` route désormais réellement sur `p.action`.

**Testé en conditions réelles contre AstroQuest, 4 scénarios** :
1. `action="new"` avec une URL différente (Nexus ERP) : crée bien un second onglet distinct (`list` confirme 2 pages).
2. `action="select"` sur le premier onglet : un `browser_snapshot` sans `page_id` explicite cible bien la bonne page (AstroQuest, pas Nexus ERP).
3. `action="close"` sur le second onglet : `list` confirme le retour à 1 page.
4. **Cas limite** : fermer l'onglet *actif* (pas un autre) — un appel `browser_snapshot` suivant sans `page_id` bascule automatiquement sur la page restante sans planter, plutôt que de pointer sur une référence d'onglet fermé.

Les autres tools audités dans le même passage (`browser_evaluate`, `browser_console_messages`, `browser_network_requests`, `browser_screenshot`, `browser_wait_for`, `browser_report`, `browser_select_option` sur une ref invalide) se sont tous comportés correctement dès le premier essai — aucun autre gap de ce type détecté cette fois.

### Complément d'audit : `browser_select_option` et `browser_file_upload` sur des éléments réels

Les deux derniers tools jamais exercés (testés jusqu'ici uniquement sur une ref invalide, cas d'erreur) ont été vérifiés sur une page HTML de test avec un vrai `<select>` à 3 options et un vrai `<input type="file">`, tous deux avec un listener JS qui reflète le changement dans le DOM :

- **`browser_select_option`** : sélectionner "Allemagne" (`value="de"`) sur un `<select>` initialement sur "France" produit le diff structurel exact attendu — `[e1] option "France"` perd son flag `[selected]` (introduit en V8), `[e2] option "Allemagne"` le gagne, et le texte affiché par le listener passe de `"France"` à `"de"`. Les propriétés d'état de la V8 démontrent ici toute leur valeur pratique : le changement de sélection est immédiatement lisible dans le diff, sans avoir à comparer manuellement deux arbres complets.
- **`browser_file_upload`** : upload d'un fichier `.txt` de test vers un `<input type="file">` — le snapshot qui suit confirme `"1 fichier(s): test-upload-file.txt"`, le nom exact du fichier transmis remonté correctement par le listener `change` de la page.

Aucun gap trouvé sur ces deux tools — contrairement aux deux découvertes précédentes (recipes en V9, `browser_tabs` en V11), ceux-ci fonctionnaient déjà correctement. L'audit systématique des tools jamais testés touche maintenant à sa fin : les 21 tools du catalogue ont tous été exercés au moins une fois en conditions réelles au cours de cette session.

## V13 — vérification (aucun code changé, lacune de documentation corrigée)

### Le multi-session simultané fonctionne, mais son mode d'emploi n'était jamais documenté

Le multi-session (`SPECTRA_MAX_SESSIONS`, "simuler du multi-utilisateur") est présenté comme une capacité clé depuis la V1, mais jamais réellement testé avec deux sessions **actives en parallèle** dans cette session de travail — jusqu'à un test dédié : lancer deux projets (`user_a`, `user_b`), naviguer chacun vers une page différente, et vérifier qu'agir sur l'un n'affecte jamais l'état de l'autre.

**Découverte en creusant le code avant de tester** : aucun tool à part `browser_launch` et `browser_run_recipe` n'accepte de paramètre `project` explicite (vérifié dans `PageScopedParams`/`RefParams` — aucun champ de ce nom). Tous les autres tools (`browser_snapshot`, `browser_click`, etc.) opèrent systématiquement sur le "projet par défaut" du serveur — un état global qui change à chaque appel explicite de `browser_launch(project=...)`. Une lecture rapide du code aurait pu laisser croire à un vrai défaut structurel empêchant le pilotage simultané de deux sessions.

**Testé pour vérifier, et le comportement réel est correct** : après `browser_launch(project=user_a)` → navigation → `browser_launch(project=user_b)` → navigation vers une autre page → `browser_launch(project=user_a)` de nouveau, un `browser_snapshot` sans paramètre retourne bien l'état de `user_a` tel qu'il l'avait laissé (`http://localhost:8080/`), pas affecté par les actions effectuées sur `user_b` entre-temps. Chaque session garde son propre état (page active, refs, historique) — seul le "projet par défaut" (quel tool cible quelle session quand aucun `page_id`/`project` n'est précisé) est un état partagé, pas les sessions elles-mêmes.

**Le vrai problème était documentaire, pas fonctionnel** : ni le README ni cette page n'expliquaient jamais ce pattern d'usage (rappeler `browser_launch(project=...)` avant chaque bloc d'actions pour basculer de session) — un agent pouvait raisonnablement s'attendre à un paramètre `project` disponible partout, qui n'existe pas. Corrigé en ajoutant un exemple concret dans le README (section "Gestion multi-session").

## V14 — vérifications de robustesse (aucun gap trouvé)

Deux scénarios de robustesse jamais testés explicitement cette session, vérifiés par précaution après la série de corrections V9/V11 :

### Erreurs réseau

- **Port fermé** (`http://localhost:59999/nonexistent`) : erreur claire et actionnable en 2,84s (`net::ERR_CONNECTION_REFUSED` transmis tel quel depuis Chrome), pas de blocage.
- **Domaine DNS invalide** (`http://this-domain-does-not-exist-xyz123.invalid/`) : erreur quasi immédiate (0,12s, `net::ERR_NAME_NOT_RESOLVED`) — la résolution DNS échoue avant même d'atteindre le timeout réseau.
- **Le serveur reste stable après chaque échec** : un `browser_navigate` normal vers AstroQuest juste après chacune de ces deux erreurs fonctionne parfaitement, sans étape de récupération nécessaire — pas d'état corrompu laissé par l'échec précédent.

### Rafale d'actions consécutives

10 appels `browser_snapshot(detail="refs_only")` enchaînés le plus vite possible (sans délai entre chaque, testant explicitement l'absence de race condition sur l'accès concurrent aux registres internes — refs, buffers console/réseau) : **10/10 réussis en 0,74s au total** (~74ms par appel), aucune erreur, aucun résultat corrompu ou désynchronisé.

Aucun nouveau gap trouvé dans cette passe — signe que le projet atteint un état de maturité raisonnable après les corrections des V9/V11, plutôt qu'un signe qu'il n'y avait rien à chercher (l'audit exhaustif des tools en V11 avait déjà, lui, trouvé deux bugs majeurs).

## V15 — améliorations livrées

### Messages d'erreur `browser_evaluate` nettoyés sur exception JS

En testant `browser_evaluate` sur une dizaine de cas limites (`undefined`, `null`, `NaN`, `Infinity`, exception non catchée, promesse non attendue, objet DOM, fonction, référence circulaire, tableau normal), la plupart se sont comportés de façon standard et attendue (les valeurs non sérialisables en JSON deviennent `null`, cohérent avec `serde_json`). Un cas ressortait cependant comme du bruit technique pur : une expression qui lève une exception JS retournait le `Debug` Rust brut de la structure CDP (`ExceptionDetails { exception_id: 1, text: "Uncaught", line_number: 0, script_id: Some(ScriptId("1000")), stack_trace: Some(StackTrace {...`), illisible pour un agent qui a juste besoin de savoir ce que l'exception disait.

Corrigé en interceptant spécifiquement `CdpError::JavascriptException` et en extrayant le message + la localisation, avec le même pattern déjà éprouvé côté `spawn_exception_listener` (observers.rs, V3 — la détection d'exceptions non catchées dans la page). Avant : un pavé de `Debug` Rust. Après : `"l'expression a levé une exception: Error: test error\n    at <anonymous>:1:16\n    at <anonymous>:1:42"` — le message et la stack trace JS, rien de plus.

**Un cas limite noté mais non corrigé, documenté honnêtement** : `Promise.resolve(42)` comme expression retourne `42` (la valeur résolue), pas la représentation d'un objet Promise en attente. C'est en fait le comportement correct de `Runtime.evaluate` avec `awaitPromise` implicite côté chromiumoxide — pas un bug, juste un comportement qui pourrait surprendre un agent qui s'attendrait à voir un objet `Promise {<pending>}` comme dans une console DevTools interactive. Documenté ici plutôt que "corrigé" vers un comportement moins utile.

## V16 — améliorations livrées

### Le message d'erreur de `SPECTRA_MAX_SESSIONS` pointait vers le mauvais tool

Jamais réellement déclenché en conditions réelles cette session (les tests précédents restaient sous la limite), donc testé explicitement avec `SPECTRA_MAX_SESSIONS=2` et 3 projets distincts. Le comportement de base fonctionne (la limite est bien respectée, la 3ᵉ session refusée avec un message explicite), mais ce message suggérait `browser_tabs action="close"` pour libérer un slot — **le mauvais tool** : `browser_tabs` ferme un onglet au sein d'une session déjà ouverte, pas une session Chrome entière. Un agent qui suivrait ce conseil à la lettre appellerait le mauvais outil et resterait bloqué.

Corrigé pour pointer vers `browser_sessions(action="close", project="...")`, le vrai tool qui ferme une session complète. **Testé le flux de récupération complet** : limite atteinte (2/2) → message d'erreur → `browser_sessions(close)` sur une des deux sessions actives → nouveau `browser_launch` réussi. Chaque étape suit désormais exactement ce que le message d'erreur recommande.

Confirmé au passage : le warm-up pré-chauffé (V4) n'est pas compté dans `SPECTRA_MAX_SESSIONS` — avec la limite à 2, deux sessions explicites + le warm-up du cwd coexistent (3 entrées dans `browser_sessions(list)`), cohérent avec le compromis déjà documenté en V4 (dépassement transitoire borné, jamais permanent).

## V17 — améliorations livrées

### Vraie fuite de process Node corrigée sur timeout d'une recipe

En lisant le code de `recipe.rs` par précaution (dans la lignée de l'attention portée aux fuites de process depuis le début de cette session), un vrai risque a sauté aux yeux : `tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())` — `wait_with_output()` **consomme** le `Child` (prend `self`). Si le timeout expire pendant cet await, le future est droppé, mais le `Child` a déjà été déplacé à l'intérieur : aucun handle ne reste pour appeler `child.kill()`. Et `tokio::process::Child` n'active pas `kill_on_drop` par défaut (comportement Tokio standard, pour rester cohérent avec un shell classique).

**Confirmé par un test isolé plutôt que supposé** : une recipe qui ne se termine jamais (`await new Promise(() => {})`, aucune résolution possible) déclenche bien le timeout de 30s côté agent — mais `tasklist`/`Get-CimInstance` montrent le process `node.exe recipe-runner.mjs` toujours vivant après. Sans intervention manuelle, ce process serait resté bloqué indéfiniment (sa Chrome piloté via CDP restant lui aussi ouvert tant que le process Node existe).

**Corrigé** en restructurant `recipe.rs` pour ne jamais passer `child` par valeur : lecture de stdout/stderr séparément via `AsyncReadExt::read_to_end` (en parallèle, `tokio::join!`) puis `child.wait()`, le tout à l'intérieur du `tokio::time::timeout` — si le timeout se déclenche, `child` reste accessible et `child.kill().await` est appelé explicitement avant de retourner l'erreur.

**Testé après correction, les deux cas** :
- Recipe normale (`ping`, retour immédiat) : fonctionne à l'identique, `0,17s`, aucune régression.
- Recipe infinie : timeout toujours à ~30s, message d'erreur désormais explicite ("le process a été arrêté"), et **vérifié directement via `Get-CimInstance`** qu'aucun process `node.exe recipe-runner.mjs` ne survit après le timeout — la fuite est éliminée, pas seulement le message d'erreur amélioré.

## V18 — vérification (aucun code changé, comportement déjà correct confirmé)

### Récupération après un crash brutal du serveur

Testé un scénario jamais vérifié explicitement cette session : que se passe-t-il si le process `spectra-server.exe` est tué brutalement (`kill -9`, pas un arrêt propre qui laisse le `Drop` de `BrowserSession` s'exécuter) pendant qu'un Chrome est actif ? Le Chrome enfant, lui, survit forcément — un `kill -9` du parent ne tue pas les enfants sous Windows.

**Séquence testée** : premier serveur lance un Chrome sur un projet → `kill -9` brutal du serveur → un second serveur est démarré sur le **même projet** → `browser_launch` s'exécute.

**Résultat : récupération immédiate et propre**, sans aucune intervention manuelle. Le second serveur détecte via `acquire_lock` (lecture du lockfile + vérification `process_alive` sur le PID enregistré) que l'ancien serveur est bien mort, et via `probe_existing` (sonde HTTP sur le port CDP dérivé) que le Chrome, lui, est toujours vivant — il s'y attache directement (`attached_existing: true`) en 0,00s au lieu d'en spawn un nouveau. Un `browser_navigate` qui suit fonctionne normalement sur cette session récupérée.

**Un vrai compromis identifié en creusant, pas un bug** : ce second serveur, une fois attaché, ne "possède" pas ce Chrome (`owns_process: false`) — par design (voir le commentaire existant sur `Drop`), il ne le tuera donc jamais, même via `browser_sessions(close_all)`. Un Chrome orphelin issu d'un crash peut donc rester ouvert indéfiniment tant qu'aucun serveur explicite ne le referme. **Vérifié que le filet de sécurité existe malgré tout** : `scripts/kill-all-spectra.ps1` filtre uniquement par ligne de commande (`--remote-debugging-port` + chemin de profil Spectra), sans notion d'"attachement" (un concept qui n'existe qu'en mémoire côté process serveur) — donc il nettoie bien ce genre de Chrome orphelin en dernier recours, quel que soit son historique d'attachement.

## V19 — améliorations livrées

### Bug majeur : `headless` et `mode` de `browser_launch` étaient silencieusement ignorés

**La découverte la plus significative de cette session sur le plan de la confiance dans les tests précédents.** En testant explicitement `mode="attach"` strict sans Chrome existant (jamais vérifié jusqu'ici — cette combinaison précise n'était jamais apparue dans les tests antérieurs), le résultat retourné était `status: "ok"` au lieu de l'erreur attendue. En creusant le code : `browser_launch` accepte bien `mode` et `headless` dans son schéma MCP (`LaunchParams`, documentés dans le README depuis la V1), mais `session_for()` — la fonction interne appelée par `browser_launch` et par tous les autres tools — appelait **systématiquement** `BrowserSession::launch(key, LaunchMode::Auto, false)`, en dur. Ni `p.mode` ni `p.headless` n'étaient jamais lus, nulle part dans le fichier.

**Conséquence concrète, vérifiée** : `mode="attach"` ne pouvait jamais échouer comme prévu, même sans Chrome existant, un nouveau était toujours spawné à la place. Et — plus impactant — **`headless=true` n'a jamais eu d'effet réel** : vérifié en inspectant la ligne de commande du Chrome lancé (`Get-CimInstance Win32_Process`), aucun flag `--headless` n'apparaissait malgré la demande explicite. Concrètement, **tous les Chrome de test lancés cette session avec `headless: true`** (la quasi-totalité des tests documentés dans ce fichier, de la V1 à la V18) **ont en réalité tourné avec une fenêtre, jamais en vrai mode headless CDP** — invisible en pratique sur cette machine faute de focus automatique de la fenêtre, ce qui explique pourquoi ce bug n'avait jamais été repéré malgré des dizaines de sessions de test.

**Corrigé** en ajoutant `session_for_with_options(project, mode, headless)`, qui transmet réellement ces valeurs à `BrowserSession::launch` — utilisé uniquement par `browser_launch` (les autres tools continuent d'utiliser `session_for()`, qui délègue avec les valeurs par défaut `Auto`/`false`, cohérent avec le fait qu'ils ne créent jamais de session eux-mêmes, seulement `browser_launch` le fait explicitement).

**Testé après correction** :
- `browser_launch(headless=true)` : la commandline Chrome contient désormais bien `--headless --hide-scrollbars --mute-audio` — vérifié directement, pas juste supposé du retour JSON.
- `browser_launch(mode="attach")` sans Chrome existant : échoue maintenant proprement (`"aucune instance Chrome trouvée sur le port ... (mode attach strict)"`), et `mode="auto"` sur le même projet ensuite récupère normalement.
- Non-régression complète vérifiée (single-tab, Core Web Vitals, diff structurel) — ce fix ne change le comportement que quand `mode`/`headless` sont explicitement demandés, jamais le chemin par défaut des autres tools.

**Ce que ça ne change pas rétroactivement** : les mesures de performance/RAM documentées dans les sections précédentes (V1 à V18) restent valides — elles comparaient un Chrome "avec tête mais sans interaction visuelle" à Playwright, pas un vrai headless dégradé. Le delta RAM mesuré en V10 (Chromium ultra-configuré) reste également valable dans l'absolu, indépendamment de ce bug.

## V20 — audit (résultat rassurant, aucun code changé)

### Audit systématique : le bug `mode`/`headless` de la V19 était-il isolé ?

Après avoir trouvé trois bugs du même acabit cette session (un paramètre exposé dans le schéma MCP, documenté, mais silencieusement ignoré côté implémentation — recipes en V9, `browser_tabs` en V11, `mode`/`headless` en V19), un doute raisonnable : combien d'autres paramètres dorment dans `params.rs` sans jamais être lus ?

**Méthode** : lister tous les champs de toutes les structs de `params.rs` (`format`, `full_page`, `level`, `on_error`, `op`, `status_filter`, `submit`, `timeout_ms`, `wait_until`, `since_ts`, `label`, et les autres déjà vérifiés dans les sections précédentes), puis vérifier pour chacun qu'il apparaît bien dans `server.rs` — soit directement (`p.xxx`), soit via l'itération sur une sous-structure (`action.xxx` pour les champs de `SequenceAction`, utilisés dans `browser_act_sequence`).

**Résultat : aucun autre champ orphelin trouvé.** Chaque paramètre du catalogue est bien lu au moins une fois. Vérifié aussi que les valeurs par défaut appliquées via `unwrap_or`/`unwrap_or_else` correspondent à ce que les descriptions `schemars` documentent (ex: `wait_until` absent ou différent de `"networkidle"` retombe bien sur le mode DOM-settled adaptatif documenté comme défaut ; `format` absent ou invalide sur `browser_screenshot` retombe silencieusement sur PNG plutôt que de planter).

Ce résultat négatif est documenté ici pour la même raison que les vérifications de robustesse des V14/V18 : un audit qui ne trouve rien reste une information utile — il borne le risque restant plutôt que de le laisser dans le flou après une découverte aussi significative que celle de la V19.

## V21 — vérification (aucun code changé)

### Les mesures de performance sont-elles affectées par le bug `headless` de la V19 ?

Question directe suite à la découverte de la V19 : puisque tous les Chrome de test cette session tournaient en fait "avec tête" (fenêtre non rendue visible faute de focus automatique, mais bien un vrai process de rendu, pas un headless CDP), les chiffres de RAM et de latence documentés dans les sections précédentes sont-ils faussés ?

**Remesuré avec le vrai headless (fix V19 appliqué)** :
- **RAM Chrome** : 1 174,65 Mo sur une session AstroQuest complète (navigate + 3s de stabilisation) — cohérent avec la fourchette déjà mesurée en V10 (1 182–1 236 Mo selon la variante de flags), pas d'écart significatif.
- **Latence `navigate`** : 3,42s — cohérent avec les 3,38s déjà documentés en V8.

**Conclusion honnête** : sur cette machine et cette page précise, le bug `headless` ignoré n'a pas faussé les chiffres de façon perceptible. L'explication la plus probable : `--disable-gpu` (déjà actif depuis la V10, indépendamment du bug `headless`) supprime déjà l'essentiel du coût de rendu GPU qui différencie normalement le plus un Chrome "avec tête" d'un vrai headless — la fenêtre existait techniquement, mais son contenu n'était jamais composé par un vrai pipeline de rendu GPU actif. Les chiffres déjà publiés dans ce document restent donc représentatifs, sans qu'il soit nécessaire de les corriger rétroactivement.

## V22 — améliorations livrées

### `browser_wait_for(ref=...)` : documenté depuis la V1, jamais réellement câblé

**Cinquième bug du même acabit cette session** (après recipes en V9, `browser_tabs` en V11, `mode`/`headless` en V19) — celui-ci encore plus flagrant : `WaitForParams` porte un champ `r#ref` documenté ("Attendre que cette ref existe/soit visible") depuis la toute première version, mais `browser_wait_for` (côté `server.rs`) ne le lisait jamais, et pire — la fonction interne `wait_for()` (côté `actions.rs`) **n'acceptait même pas ce paramètre en entrée**. Le concept "attendre qu'une ref soit visible" n'existait tout simplement nulle part dans le code, malgré la documentation.

**Implémenté réellement** : une ref est considérée "visible" quand son `backend_node_id` résout un `DOM.getBoxModel` valide (l'élément a une géométrie concrète dans la page, donc il est rendu) — un signal plus fort que `resolve_ref` seul, qui ne garantit que la présence de la ref dans le registre en mémoire du dernier snapshot, pas que l'élément existe encore réellement dans le DOM à l'instant présent. Si `text` et `ref` sont tous les deux fournis, les deux doivent être satisfaits (cohérent avec une lecture naturelle de "attendre CE texte ET cette ref").

**Testé en conditions réelles contre AstroQuest, 4 scénarios** :
1. Ref déjà visible au moment de l'appel : `satisfied=true` en 0,11s.
2. Ref inexistante (jamais vue dans un snapshot) : timeout propre à 2,09s (proche du `timeout_ms=2000` demandé), pas de plantage.
3. `ref` et `text` combinés, tous deux déjà satisfaits : `satisfied=true` en 0,14s.
4. Ref d'un élément qui vient d'apparaître dynamiquement (combobox de la modale Spotlight, ouverte juste avant) : résolue correctement.

Non-régression vérifiée sur le reste (single-tab, Core Web Vitals, diff structurel) — le changement de signature de `wait_for()` (ajout d'un paramètre) ne touche qu'à ce tool précis.

## V23 — audit final (aucun code changé)

### Clôture de la classe de bug "paramètre documenté mais ignoré"

Après cinq corrections de ce type cette session (recipes en V9, `browser_tabs` en V11, `mode`/`headless` en V19, `browser_wait_for(ref=...)` en V22), un dernier passage exhaustif sur l'intégralité de `params.rs` — chaque struct, chaque champ — pour confirmer qu'il n'en reste plus :

- Tous les champs déjà couverts par l'audit V20 (`format`, `full_page`, `level`, `on_error`, `op`, `status_filter`, `submit`, `timeout_ms`, `wait_until`, `since_ts`, `label`) restent valides après les changements V22.
- `ReportParams.project` : vérifié utilisé (`resolve_project_key(p.project.clone())`).
- `page_id` : compté 18 usages (`p.page_id`) pour 16 déclarations dans `params.rs` — écart expliqué (pas suspect) par `act_on_ref`/`act_on_ref_named`, qui réutilisent la même valeur deux fois dans leur code interne.

**Aucun nouveau bug trouvé.** Cette classe de problème — un champ exposé dans le schéma MCP et documenté, mais jamais réellement lu par l'implémentation — semble maintenant close pour ce catalogue de 21 tools. Les cinq corrections livrées cette session couvraient l'intégralité des cas réels : rien ne dort plus silencieusement dans le code.

## V24 — vérification de performance (aucun code changé, non-problème confirmé)

### Le ring buffer console/réseau (`Vec::remove(0)`, O(n)) n'a pas d'impact mesurable

En lisant `console.rs`/`network.rs` par précaution, un détail d'implémentation attire l'œil : l'éviction du plus ancien message quand la capacité de 500 est dépassée utilise `self.order.remove(0)` sur un `Vec` — une opération O(n) (tous les éléments suivants sont décalés), alors qu'un `VecDeque` offrirait un retrait en tête O(1). Une optimisation en apparence évidente.

**Testé avant de "corriger" un problème qui n'existait peut-être pas** : génération de 1000 messages console **distincts** (donc non-dédupliqués, forçant 500 évictions réelles) via `browser_evaluate`, en une seule rafale. Résultat : **47ms pour générer les 1000 messages, 4ms pour les relire** — aucun ralentissement perceptible. À l'échelle de 500 éléments, le coût O(n) d'un `Vec::remove(0)` reste de l'ordre de la microseconde ; le remplacer par un `VecDeque` n'aurait aucun effet mesurable en pratique, seulement un gain théorique invisible à cette taille.

**Décision : ne pas toucher au code.** C'est le même principe déjà appliqué dans ce projet (voir la discipline "abandonner plutôt qu'insister" documentée en V2/V3) appliqué dans l'autre sens : ne pas non plus "corriger" ce qui n'est pas cassé, juste parce que ça a l'air théoriquement sous-optimal sur le papier. Documenté ici pour que la question ne se repose pas sans données à l'avenir.

## V25 — améliorations livrées

### Revue de cohérence du README + prérequis Node incorrect corrigé

Après 24 vagues de changements, une relecture attentive du README (pas juste `docs/COMPARISON.md`, qui a reçu toute l'attention jusqu'ici) a révélé une incohérence significative : le prérequis annoncé était "Node.js 18+", mais `recipe-runner.mjs` (écrit en V9) utilise le `WebSocket` global natif de Node — une API stable seulement depuis **Node 22**. Sur une version 18 à 21, `browser_run_recipe` aurait échoué avec `ReferenceError: WebSocket is not defined`, un message opaque sans rapport apparent avec la vraie cause (une version de Node trop ancienne).

**Corrigé en deux temps** :
1. **Documentation** : le prérequis annoncé passe à "Node.js 22+", avec l'explication de pourquoi (cohérence avec le principe déjà appliqué dans ce projet de ne jamais laisser un prérequis flou ou faux).
2. **Vérification de version ajoutée côté code** : `check_node_version()` exécute `node --version` avant de lancer la vraie recipe, et échoue avec un message actionnable si la version détectée est antérieure à 22 — même discipline de "messages d'erreur actionnables" déjà appliquée au démarrage de Chrome (V3) et aux exceptions JS de `browser_evaluate` (V15).

**Testé** : sur cette machine (Node 22.18.0), la vérification passe silencieusement et une recipe simple s'exécute normalement (`{"pong": true}`) — aucune régression sur le chemin nominal. La vérification elle-même n'a pas pu être testée sur une vraie version antérieure de Node (aucune disponible sur cette machine pour un test destructif), mais la logique de comparaison (parsing de `v22.18.0` → `22`, comparaison `< 22`) est simple et directement vérifiable par lecture.

**Autres corrections de cohérence du README dans le même passage** : mention du dossier `runtime/` (absent du schéma d'architecture malgré son existence depuis la V9), description de `browser_wait_for` mise à jour pour refléter le support réel de `ref` (V22, auparavant juste "attend... une ref" sans plus de détail alors que ce n'était même pas câblé), remplacement d'une référence obsolète à une section "Roadmap V2" qui n'existe plus telle quelle dans `docs/COMPARISON.md`.

## V26 — améliorations livrées

### `browser_network_request_detail` : les headers HTTP promis par la doc n'existaient pas

En testant ce tool en profondeur (jamais fait avant cette session), une lacune similaire à celle du prérequis Node en V25 : le catalogue promettait "Détail complet d'une requête (**headers**, statut)" depuis la V1, mais `RequestRecord` ne contenait jamais de headers — seulement `request_id`, `url`, `method`, `status`, `domain`, `status_text`, `failed_reason`, `long_lived`. Contrairement aux bugs de paramètres ignorés (V9/V11/V19/V22), ici le code faisait exactement ce qu'il faisait ; c'est la documentation qui promettait quelque chose de jamais implémenté.

**Corrigé en capturant ce qui était déjà disponible mais jeté** : les événements CDP déjà écoutés (`EventRequestWillBeSent`, `EventResponseReceived`) portent `request.headers`/`response.headers` (type CDP `Headers`, un simple wrapper `serde_json::Value`) — il suffisait de les stocker au lieu de les ignorer. Ajout de deux champs optionnels `request_headers`/`response_headers` à `RequestRecord`.

**Testé en conditions réelles contre AstroQuest** : le détail d'une requête réelle (un chunk Vite) retourne désormais les vrais headers — `User-Agent`, `Origin`, `Referer`, `sec-ch-ua` côté requête ; `Content-Type`, `Cache-Control`, `Access-Control-Allow-Origin`, `Content-Length` côté réponse. Effet de bord positif : le `User-Agent` capturé confirme `HeadlessChrome/149.0.0.0`, une preuve supplémentaire (indépendante des vérifications déjà faites en V19/V21) que le vrai mode headless fonctionne bien sur le binaire actuel. Non-régression vérifiée sur le reste (single-tab, Core Web Vitals, diff structurel).

**Déployé et revérifié sur le binaire release** (`~/.spectra/bin/spectra-server.exe`, build terminé en 1m28s) : le comportement ci-dessus (headers réels, cas d'erreur `request_id` invalide bien géré) est identique en release. Suite de non-régression complète repassée sur ce binaire : `browser_snapshot` (accessibility tree cohérent), `browser_performance_metrics` (Core Web Vitals plausibles : `fcp_ms`, `lcp_ms`, `cls`, `ttfb_ms`), diff structurel (deuxième snapshot après interaction), `browser_wait_for(ref=...)`, `browser_run_recipe` (vérification Node 22 + exécution réelle, refs et texte cohérents) — tous OK.

**Faux positif rencontré puis écarté pendant cette vérification** : un premier `browser_navigate` a échoué avec `Request timed out` juste après un `browser_launch` frais, pendant une fenêtre où la machine venait de terminer une compilation release et faisait tourner plusieurs Chrome de test accumulés (16 process orphelins retrouvés au nettoyage, jamais capturés par les kills précédents qui ne ciblaient que le PID direct sans les enfants déjà détachés). Reproduit à l'identique sur le binaire **debug** (donc pas lié à ce build) puis re-testé sur le binaire release une fois la machine calme et le nettoyage fait : `browser_navigate` réussit alors de façon fiable et rapide (~5-6s à froid, ~3s ensuite). Conclusion : contention machine transitoire, pas une régression de code — mais leçon retenue sur la discipline de nettoyage (voir note ci-dessous).

**Note opérationnelle retenue** : `taskkill /T /F /PID <pid>` sur le process Chrome parent ne suffit pas toujours à emporter tous les sous-process si certains se sont déjà détachés du parent (erreurs "processus introuvable" observées sur des PID enfants pourtant bien vivants juste avant). Le filtre par `CommandLine` (`*Spectra\profiles*` + `*--remote-debugging-port*`) reste la bonne méthode — l'accumulation venait de nettoyages répétés au fil de la session sans repasser régulièrement ce filtre large, pas d'un défaut du filtre lui-même.
