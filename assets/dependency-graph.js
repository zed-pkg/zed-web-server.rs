const SVG_NS = "http://www.w3.org/2000/svg";
const NODE_WIDTH = 224;
const NODE_HEIGHT = 64;
const SCOPE_LIMIT = 80;
const FORCE_LIMIT = 260;
const MAX_GRAPH_NODES = 3000;
const MAX_GRAPH_EDGES = 12000;
const HTMLElementBase = globalThis.HTMLElement || class {};

const KIND_LABELS = {
  runtime: "Runtime",
  build: "Build",
  development: "Development",
  peer: "Peer",
  tooling: "Tooling",
};

const EXPORT_FORMATS = [
  ["json", "JSON"],
  ["yaml", "YAML"],
  ["toml", "TOML"],
  ["json5", "JSON5 + comments"],
  ["xml", "XML"],
  ["csv", "CSV edge list"],
  ["msgpack", "MessagePack"],
  ["protobuf", "Protocol Buffers"],
  ["dot", "Graphviz DOT"],
  ["mermaid", "Mermaid"],
];

class ZedDependencyGraph extends HTMLElementBase {
  constructor() {
    super();
    this.mode = "package";
    this.nodes = new Map();
    this.edges = [];
    this.edgeByKey = new Map();
    this.renderedNodeElements = new Map();
    this.renderedEdgesByNode = new Map();
    this.roots = new Set();
    this.positions = new Map();
    this.selectedId = null;
    this.pathStartId = null;
    this.focusNodes = null;
    this.focusEdges = null;
    this.focusLabel = "";
    this.searchTerm = "";
    this.layoutName = "layered";
    this.enabledKinds = new Set(Object.keys(KIND_LABELS));
    this.includeOptional = true;
    this.transform = { x: 0, y: 0, k: 1 };
    this.drag = null;
    this.cache = new Map();
    this.loadSequence = 0;
    this.sourceFailures = 0;
    this.syntheticTraversal = false;
    this.resizeObserver = null;
    this.handleWindowPointerMove = (event) => this.onPointerMove(event);
    this.handleWindowPointerUp = (event) => this.onPointerUp(event);
    this.handleHashChange = () => this.restoreSelectionFromHash();
    this.searchFrame = null;
  }

  connectedCallback() {
    if (this.dataset.ready === "true") return;
    this.dataset.ready = "true";
    this.mode = this.dataset.mode || "package";
    this.versions = parseJson(this.dataset.versions, []);
    this.sources = parseJson(this.dataset.sources, []);
    this.layoutName = safeStorageGet("zpkg.graph.layout") || "layered";
    if (!["layered", "radial", "force"].includes(this.layoutName)) {
      this.layoutName = "layered";
    }
    this.renderShell();
    this.bindControls();
    this.loadInitial();
  }

  disconnectedCallback() {
    window.removeEventListener("pointermove", this.handleWindowPointerMove);
    window.removeEventListener("pointerup", this.handleWindowPointerUp);
    window.removeEventListener("hashchange", this.handleHashChange);
    this.resizeObserver?.disconnect();
    if (this.searchFrame !== null) cancelAnimationFrame(this.searchFrame);
    this.resizeObserver = null;
    this.searchFrame = null;
    delete this.dataset.ready;
  }

  renderShell() {
    const scopeTitle = escapeHtml(this.dataset.scopeTitle || "Dependency topology");
    const scopeDescription = escapeHtml(
      this.dataset.scopeDescription ||
        "Explore package relationships, focus common queries, and open any package without leaving the graph."
    );
    const versionOptions = this.versions
      .map((item) => {
        const badges = [item.prerelease ? "pre-release" : "", item.yanked ? "yanked" : ""]
          .filter(Boolean)
          .join(", ");
        const label = badges ? `${item.version} · ${badges}` : item.version;
        const selected = item.version === this.dataset.version ? " selected" : "";
        return `<option value="${escapeHtml(item.version)}"${selected}>${escapeHtml(label)}</option>`;
      })
      .join("");

    this.innerHTML = `
      <section class="dg-shell" aria-label="Dependency graph workspace">
        <header class="dg-header">
          <div>
            <p class="dg-eyebrow">Interactive dependency intelligence</p>
            <h2>${scopeTitle}</h2>
            <p class="dg-subtitle">${scopeDescription}</p>
          </div>
          <div class="dg-metrics" aria-live="polite">
            <span><strong data-metric="nodes">0</strong> packages</span>
            <span><strong data-metric="edges">0</strong> relationships</span>
            <span><strong data-metric="depth">0</strong> depth</span>
          </div>
        </header>

        <div class="dg-toolbar" role="toolbar" aria-label="Graph controls">
          ${
            this.mode === "package"
              ? `<label class="dg-field dg-version-field">
                   <span>Version</span>
                   <select data-control="version">${versionOptions}</select>
                 </label>`
              : ""
          }
          <label class="dg-field dg-search-field">
            <span>Find package</span>
            <input data-control="search" type="search" placeholder="org/name or version" autocomplete="off">
          </label>
          <div class="dg-segmented" aria-label="Graph layout">
            ${["layered", "radial", "force"]
              .map(
                (layout) =>
                  `<button type="button" data-layout="${layout}" aria-pressed="${
                    this.layoutName === layout
                  }">${capitalize(layout)}</button>`
              )
              .join("")}
          </div>
          <button type="button" class="dg-icon-button" data-action="fit" title="Fit graph">Fit</button>
          <button type="button" class="dg-icon-button" data-action="reset" title="Reset filters and focus">Reset</button>
          ${this.mode === "package" ? this.exportMenu() : ""}
        </div>

        <div class="dg-querybar" role="toolbar" aria-label="Common dependency queries">
          <span class="dg-query-label">Queries</span>
          <button type="button" data-query="direct">Direct dependencies</button>
          <button type="button" data-query="transitive">Transitive dependencies</button>
          <button type="button" data-query="dependents">Reverse impact</button>
          <button type="button" data-query="cycles">Cycles</button>
          <button type="button" data-query="longest">Longest chain</button>
          <button type="button" data-query="pin-path">Pin path start</button>
          <button type="button" data-query="shortest">Shortest path</button>
          <button type="button" data-query="clear">Clear focus</button>
          <details class="dg-filter-menu">
            <summary>Edge filters</summary>
            <div class="dg-filter-panel">
              ${Object.entries(KIND_LABELS)
                .map(
                  ([kind, label]) => `<label>
                    <input type="checkbox" data-kind="${kind}" checked> ${label}
                  </label>`
                )
                .join("")}
              <label><input type="checkbox" data-control="optional" checked> Optional edges</label>
            </div>
          </details>
        </div>

        <div class="dg-notice" data-role="notice" hidden></div>
        <div class="dg-status" data-role="status" aria-live="polite">Preparing graph workspace…</div>

        <div class="dg-stage">
          <div class="dg-viewport" data-role="viewport">
            <svg data-role="svg" role="group" aria-label="Interactive package dependency graph" tabindex="0">
              <defs>
                <marker id="dg-arrow" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto" markerUnits="strokeWidth">
                  <path d="M0,0 L9,4.5 L0,9 z"></path>
                </marker>
                <filter id="dg-glow" x="-40%" y="-40%" width="180%" height="180%">
                  <feGaussianBlur stdDeviation="4" result="blur"></feGaussianBlur>
                  <feMerge><feMergeNode in="blur"></feMergeNode><feMergeNode in="SourceGraphic"></feMergeNode></feMerge>
                </filter>
              </defs>
              <g data-role="world">
                <g data-role="edges"></g>
                <g data-role="nodes"></g>
              </g>
            </svg>
            <div class="dg-empty" data-role="empty" hidden>
              <strong>No relationships match this view.</strong>
              <span>Clear the query or enable more edge kinds.</span>
            </div>
            <div class="dg-zoom-readout" data-role="zoom">100%</div>
          </div>

          <aside class="dg-inspector" data-role="inspector" aria-label="Selected package details" aria-live="polite">
            <div class="dg-inspector-empty">
              <div class="dg-orbit" aria-hidden="true"></div>
              <h3>Select a package</h3>
              <p>Inspect identity, requirements, features, incoming impact, and outgoing dependencies.</p>
            </div>
          </aside>
        </div>

        <div class="dg-legend">
          ${Object.entries(KIND_LABELS)
            .map(([kind, label]) => `<span><i class="dg-key dg-kind-${kind}"></i>${label}</span>`)
            .join("")}
          <span><i class="dg-key dg-key-optional"></i>Optional</span>
          <span><i class="dg-key dg-key-synthetic"></i>Latest declared expansion</span>
        </div>

        <details class="dg-accessible">
          <summary>Accessible relationship table</summary>
          <div data-role="table"></div>
        </details>
      </section>`;

    this.$ = (selector) => this.querySelector(selector);
    this.$$ = (selector) => [...this.querySelectorAll(selector)];
    this.svg = this.$('[data-role="svg"]');
    this.world = this.$('[data-role="world"]');
    this.edgeLayer = this.$('[data-role="edges"]');
    this.nodeLayer = this.$('[data-role="nodes"]');
    this.viewport = this.$('[data-role="viewport"]');
    this.inspector = this.$('[data-role="inspector"]');
    this.status = this.$('[data-role="status"]');
    this.notice = this.$('[data-role="notice"]');
  }

  exportMenu() {
    return `<details class="dg-export-menu">
      <summary>Download</summary>
      <div>
        ${EXPORT_FORMATS.map(
          ([format, label]) =>
            `<a data-export="${format}" href="#" download>${escapeHtml(label)}</a>`
        ).join("")}
      </div>
    </details>`;
  }

  bindControls() {
    this.$('[data-control="version"]')?.addEventListener("change", (event) => {
      this.dataset.version = event.target.value;
      this.loadPackage(event.target.value);
    });

    this.$('[data-control="search"]').addEventListener("input", (event) => {
      this.searchTerm = event.target.value.trim().toLowerCase();
      if (this.searchFrame !== null) cancelAnimationFrame(this.searchFrame);
      this.searchFrame = requestAnimationFrame(() => {
        this.searchFrame = null;
        this.renderGraph();
      });
    });

    this.$$('[data-layout]').forEach((button) => {
      button.addEventListener("click", () => {
        this.layoutName = button.dataset.layout;
        safeStorageSet("zpkg.graph.layout", this.layoutName);
        this.$$('[data-layout]').forEach((item) =>
          item.setAttribute("aria-pressed", String(item === button))
        );
        this.applyLayout(true);
      });
    });

    this.$$('[data-action]').forEach((button) => {
      button.addEventListener("click", () => {
        if (button.dataset.action === "fit") this.fitGraph();
        if (button.dataset.action === "reset") this.resetView();
      });
    });

    this.$$('[data-query]').forEach((button) => {
      button.addEventListener("click", () => this.runQuery(button.dataset.query));
    });

    this.$$('[data-kind]').forEach((checkbox) => {
      checkbox.addEventListener("change", () => {
        if (checkbox.checked) this.enabledKinds.add(checkbox.dataset.kind);
        else this.enabledKinds.delete(checkbox.dataset.kind);
        this.renderGraph();
      });
    });

    this.$('[data-control="optional"]').addEventListener("change", (event) => {
      this.includeOptional = event.target.checked;
      this.renderGraph();
    });

    this.svg.addEventListener("wheel", (event) => this.onWheel(event), { passive: false });
    this.svg.addEventListener("pointerdown", (event) => this.onCanvasPointerDown(event));
    window.addEventListener("pointermove", this.handleWindowPointerMove);
    window.addEventListener("pointerup", this.handleWindowPointerUp);
    window.addEventListener("hashchange", this.handleHashChange);
    this.svg.addEventListener("keydown", (event) => {
      if (event.key === "+" || event.key === "=") this.zoomAt(1.15, this.viewport.clientWidth / 2, this.viewport.clientHeight / 2);
      if (event.key === "-") this.zoomAt(1 / 1.15, this.viewport.clientWidth / 2, this.viewport.clientHeight / 2);
      if (event.key === "0") this.fitGraph();
    });

    if (globalThis.ResizeObserver) {
      this.resizeObserver = new ResizeObserver(() => {
        if (this.nodes.size && !this.drag) this.updateTransform();
      });
      this.resizeObserver.observe(this.viewport);
    }
  }

  async loadInitial() {
    this.updateExportLinks();
    if (this.mode === "scope") await this.loadScope();
    else await this.loadPackage(this.dataset.version || this.versions[0]?.version || "");
  }

  async loadPackage(version) {
    if (!version) {
      this.setStatus("No published version is available for this package.", "error");
      return;
    }
    const sequence = ++this.loadSequence;
    this.setStatus(`Loading ${this.dataset.org}/${this.dataset.package}@${version}…`, "loading");
    this.notice.hidden = true;
    try {
      const url = packageDocumentUrl(this.dataset.org, this.dataset.package, version);
      const { document } = await this.fetchDocument(url);
      if (sequence !== this.loadSequence) return;
      this.clearGraph();
      this.addDocument(document, { primary: true, synthetic: false });
      this.dataset.version = version;
      this.syntheticTraversal = false;
      this.updateExportLinks();
      this.afterGraphLoaded(`Loaded declared graph for ${this.dataset.org}/${this.dataset.package}@${version}.`);
    } catch (error) {
      if (sequence !== this.loadSequence) return;
      this.fail(error);
    }
  }

  async loadScope() {
    const sources = this.sources.filter((source) => source.version).slice(0, SCOPE_LIMIT);
    this.clearGraph();
    this.sourceFailures = 0;
    if (!sources.length) {
      this.setStatus("No published package versions are available in this scope.", "error");
      this.renderGraph();
      return;
    }
    this.setStatus(`Loading ${sources.length} package graphs…`, "loading");
    let completed = 0;
    const loaded = new Array(sources.length);
    const { publicSources, privateSources } = scopeSourceBatches(sources);
    const loadSource = async ({ source, index }) => {
      try {
        const url = packageDocumentUrl(source.org, source.name, source.version);
        const { document } = await this.fetchDocument(url);
        loaded[index] = document;
      } catch (error) {
        this.sourceFailures += 1;
        console.warn("Dependency graph source failed", source, error);
      }
      completed += 1;
      this.setStatus(
        `Loaded ${completed} of ${sources.length} package graphs…`,
        "loading"
      );
    };
    // Private graph reads rotate the opaque browser refresh handle. Keep those
    // requests serial while loading public, anonymous graphs concurrently.
    await Promise.all([
      mapLimit(publicSources, 6, loadSource),
      mapLimit(privateSources, 1, loadSource),
    ]);
    // Apply successful documents in source order. Fetch completion order must
    // not change layout, keyboard order, or the accessible edge table.
    loaded.forEach((document, index) => {
      if (document) {
        try {
          this.addDocument(document, { primary: index === 0, synthetic: false, scopeRoot: true });
        } catch (error) {
          this.sourceFailures += 1;
          console.warn("Dependency graph source exceeded workspace limits", sources[index], error);
        }
      }
    });
    const clipped = this.sources.length > SCOPE_LIMIT;
    const notes = [];
    if (this.sourceFailures) notes.push(`${this.sourceFailures} package graph(s) could not be loaded`);
    if (clipped) notes.push(`scope limited to the first ${SCOPE_LIMIT} packages`);
    if (notes.length) this.showNotice(`${notes.join("; ")}.`, "warning");
    this.afterGraphLoaded(
      `Composed ${sources.length - this.sourceFailures} immutable declared graphs for this ${
        this.dataset.scopeKind || "scope"
      }.`
    );
  }

  clearGraph() {
    this.nodes.clear();
    this.edges = [];
    this.edgeByKey.clear();
    this.roots.clear();
    this.positions.clear();
    this.selectedId = null;
    this.pathStartId = null;
    this.focusNodes = null;
    this.focusEdges = null;
    this.focusLabel = "";
  }

  addDocument(document, options = {}) {
    if (!document || document.schema !== "zpkg/dependency-graph/v1") {
      throw new Error("The server returned an unsupported dependency graph schema.");
    }
    this.assertDocumentCapacity(document);
    if (document.view === "declared") {
      const loadedRoot = Boolean(options.primary || options.scopeRoot);
      const root = this.upsertDeclaredNode(document.package, {
        version: document.package.version,
        root: loadedRoot,
        expanded: true,
        synthetic: Boolean(options.synthetic),
      });
      if (loadedRoot) this.roots.add(root.id);
      for (const dependency of document.dependencies || []) {
        const target = this.upsertDeclaredNode(dependency, {
          requirement: dependency.requirement,
          synthetic: Boolean(options.synthetic),
        });
        this.upsertEdge({
          from: root.id,
          to: target.id,
          kind: dependency.kind || "runtime",
          requirement: dependency.requirement || "",
          target: dependency.target || "",
          optional: Boolean(dependency.optional),
          features: dependency.features || [],
          synthetic: Boolean(options.synthetic),
        });
      }
      return;
    }

    if (document.view === "resolved") {
      for (const node of document.nodes || []) {
        this.upsertResolvedNode(node, { synthetic: Boolean(options.synthetic) });
      }
      for (const rootIdentity of document.roots || []) {
        this.roots.add(resolvedKey(rootIdentity));
      }
      for (const edge of document.edges || []) {
        this.upsertEdge({
          from: resolvedKey(edge.from),
          to: resolvedKey(edge.to),
          kind: edge.kind || "runtime",
          requirement: edge.requirement || "",
          target: edge.target || "",
          optional: Boolean(edge.optional),
          features: edge.features || [],
          synthetic: Boolean(options.synthetic),
        });
      }
      return;
    }
    throw new Error("The server returned an unknown dependency graph view.");
  }

  assertDocumentCapacity(document) {
    let incomingNodes = 0;
    let incomingEdges = 0;
    if (document.view === "declared") {
      incomingNodes = 1 + (Array.isArray(document.dependencies) ? document.dependencies.length : 0);
      incomingEdges = Array.isArray(document.dependencies) ? document.dependencies.length : 0;
    } else if (document.view === "resolved") {
      incomingNodes = Array.isArray(document.nodes) ? document.nodes.length : 0;
      incomingEdges = Array.isArray(document.edges) ? document.edges.length : 0;
    } else {
      return;
    }
    // Conservative before-mutation accounting prevents a rejected source from
    // leaving a partial graph behind. Duplicate coordinates may make the
    // estimate larger than the eventual graph, which is preferable to an
    // unresponsive browser workspace.
    if (this.nodes.size + incomingNodes > MAX_GRAPH_NODES) {
      throw new Error(`The loaded topology exceeds the ${MAX_GRAPH_NODES}-package browser limit.`);
    }
    if (this.edges.length + incomingEdges > MAX_GRAPH_EDGES) {
      throw new Error(`The loaded topology exceeds the ${MAX_GRAPH_EDGES}-relationship browser limit.`);
    }
  }

  upsertDeclaredNode(identity, attributes = {}) {
    const id = coordinateKey(identity);
    const existing = this.nodes.get(id) || {
      id,
      registryId: identity.registry_id || "registry:unknown",
      org: identity.org || "unknown",
      name: identity.name || "unknown",
      version: "",
      requirements: new Set(),
      features: new Set(),
      artifactDigest: "",
      root: false,
      synthetic: false,
      resolved: false,
      expanded: false,
    };
    if (attributes.version) existing.version = attributes.version;
    if (attributes.requirement) existing.requirements.add(attributes.requirement);
    existing.root ||= Boolean(attributes.root);
    existing.expanded ||= Boolean(attributes.expanded);
    existing.synthetic ||= Boolean(attributes.synthetic);
    this.nodes.set(id, existing);
    return existing;
  }

  upsertResolvedNode(node, attributes = {}) {
    const id = resolvedKey(node.id);
    const existing = this.nodes.get(id) || {
      id,
      registryId: node.id.registry_id || "registry:unknown",
      org: node.id.org || "unknown",
      name: node.id.name || "unknown",
      version: node.id.version || "",
      requirements: new Set(),
      features: new Set(),
      artifactDigest: node.artifact_digest || "",
      root: false,
      synthetic: false,
      resolved: true,
      expanded: true,
    };
    for (const feature of node.features || []) existing.features.add(feature);
    existing.synthetic ||= Boolean(attributes.synthetic);
    this.nodes.set(id, existing);
    return existing;
  }

  upsertEdge(edge) {
    const key = edgeIdentity(edge);
    const existing = this.edgeByKey.get(key);
    if (existing) {
      existing.count += 1;
      existing.synthetic ||= edge.synthetic;
      for (const feature of edge.features || []) existing.features.add(feature);
      return existing;
    }
    const created = {
      ...edge,
      key,
      count: 1,
      features: new Set(edge.features || []),
    };
    this.edges.push(created);
    this.edgeByKey.set(key, created);
    return created;
  }

  async fetchDocument(url) {
    const cached = this.cache.get(url);
    const headers = { Accept: "application/vnd.zpkg.dependency-graph.v1+json" };
    if (cached?.etag) headers["If-None-Match"] = cached.etag;
    const response = await fetch(url, { headers, credentials: "same-origin" });
    if (response.status === 304 && cached) {
      const responseEtag = response.headers.get("etag") || "";
      const responseDigest = response.headers.get("x-zpkg-graph-digest") || "";
      if (!isStrongGraphEtag(responseEtag) || responseEtag !== cached.etag) {
        throw new Error("The dependency graph cache validator was missing or changed.");
      }
      if (!isGraphDigest(responseDigest) || responseDigest !== cached.digest) {
        throw new Error("The cached dependency graph semantic identity was missing or changed.");
      }
      return cached;
    }
    if (response.status === 304) {
      throw new Error("The dependency graph cache validator had no local representation.");
    }
    if (!response.ok) {
      const problem = await response.json().catch(() => ({}));
      throw new Error(problem.message || `Dependency graph request failed (${response.status}).`);
    }
    const contentType = response.headers.get("content-type")?.split(";", 1)[0].trim() || "";
    if (contentType !== "application/vnd.zpkg.dependency-graph.v1+json") {
      throw new Error("The server returned the wrong dependency graph representation type.");
    }
    const document = await response.json();
    const etag = response.headers.get("etag") || "";
    const headerDigest = response.headers.get("x-zpkg-graph-digest") || "";
    const documentDigest = document.graph_digest || "";
    if (!isStrongGraphEtag(etag)) {
      throw new Error("The dependency graph response did not carry a strong representation validator.");
    }
    if (
      !isGraphDigest(headerDigest) ||
      !isGraphDigest(documentDigest) ||
      headerDigest !== documentDigest
    ) {
      throw new Error("The dependency graph response carried missing or inconsistent semantic identity.");
    }
    const result = {
      document,
      etag,
      digest: headerDigest,
      selectedVersion: response.headers.get("x-zpkg-selected-version") || "",
    };
    this.cache.set(url, result);
    return result;
  }

  afterGraphLoaded(message) {
    this.applyLayout(false);
    this.updateMetrics();
    this.renderAccessibleTable();
    this.setStatus(message, "ready");
    if (!this.selectedId && !this.restoreSelectionFromHash() && this.roots.size) {
      this.selectNode([...this.roots][0], false);
    }
  }

  restoreSelectionFromHash() {
    const prefix = "#dependency-graph=";
    if (!location.hash.startsWith(prefix)) return false;
    let coordinate;
    try {
      coordinate = decodeURIComponent(location.hash.slice(prefix.length));
    } catch {
      return false;
    }
    const match = [...this.nodes.values()].find(
      (node) => `${node.org}/${node.name}` === coordinate
    );
    if (!match) return false;
    this.selectNode(match.id, false);
    return true;
  }

  applyLayout(announce = true) {
    if (!this.nodes.size) {
      this.renderGraph();
      return;
    }
    if (this.layoutName === "radial") this.layoutRadial();
    else if (this.layoutName === "force") this.layoutForce();
    else this.layoutLayered();
    this.renderGraph();
    requestAnimationFrame(() => this.fitGraph());
    if (announce) this.setStatus(`${capitalize(this.layoutName)} layout applied.`, "ready");
  }

  layoutLayered() {
    const levels = this.graphLevels();
    const grouped = new Map();
    for (const [id, level] of levels) {
      if (!grouped.has(level)) grouped.set(level, []);
      grouped.get(level).push(id);
    }
    for (const [level, ids] of [...grouped].sort((a, b) => a[0] - b[0])) {
      ids.sort((a, b) => nodeLabel(this.nodes.get(a)).localeCompare(nodeLabel(this.nodes.get(b))));
      const gap = Math.max(90, Math.min(132, 850 / Math.max(ids.length, 1)));
      ids.forEach((id, index) => {
        this.positions.set(id, {
          x: level * 292,
          y: (index - (ids.length - 1) / 2) * gap,
        });
      });
    }
  }

  layoutRadial() {
    const levels = this.graphLevels();
    const grouped = new Map();
    for (const [id, level] of levels) {
      if (!grouped.has(level)) grouped.set(level, []);
      grouped.get(level).push(id);
    }
    for (const [level, ids] of grouped) {
      ids.sort((a, b) => nodeLabel(this.nodes.get(a)).localeCompare(nodeLabel(this.nodes.get(b))));
      if (level === 0) {
        ids.forEach((id, index) => this.positions.set(id, { x: (index - (ids.length - 1) / 2) * 250, y: 0 }));
        continue;
      }
      const radius = 210 + level * 190;
      ids.forEach((id, index) => {
        const angle = (Math.PI * 2 * index) / ids.length - Math.PI / 2;
        this.positions.set(id, { x: Math.cos(angle) * radius, y: Math.sin(angle) * radius });
      });
    }
  }

  layoutForce() {
    if (this.nodes.size > FORCE_LIMIT) {
      this.layoutLayered();
      this.showNotice(
        `Force layout is capped at ${FORCE_LIMIT} packages; layered layout was used for this graph.`,
        "warning"
      );
      this.layoutName = "layered";
      safeStorageSet("zpkg.graph.layout", this.layoutName);
      this.$$('[data-layout]').forEach((button) =>
        button.setAttribute("aria-pressed", String(button.dataset.layout === this.layoutName))
      );
      return;
    }
    this.layoutLayered();
    const ids = [...this.nodes.keys()];
    const velocity = new Map(ids.map((id) => [id, { x: 0, y: 0 }]));
    for (let iteration = 0; iteration < 150; iteration += 1) {
      const cooling = 1 - iteration / 170;
      for (let i = 0; i < ids.length; i += 1) {
        const a = this.positions.get(ids[i]);
        const va = velocity.get(ids[i]);
        for (let j = i + 1; j < ids.length; j += 1) {
          const b = this.positions.get(ids[j]);
          const vb = velocity.get(ids[j]);
          let dx = a.x - b.x;
          let dy = a.y - b.y;
          const distanceSquared = Math.max(900, dx * dx + dy * dy);
          const force = (32000 / distanceSquared) * cooling;
          const distance = Math.sqrt(distanceSquared);
          dx /= distance;
          dy /= distance;
          va.x += dx * force;
          va.y += dy * force;
          vb.x -= dx * force;
          vb.y -= dy * force;
        }
      }
      for (const edge of this.edges) {
        const a = this.positions.get(edge.from);
        const b = this.positions.get(edge.to);
        if (!a || !b) continue;
        const va = velocity.get(edge.from);
        const vb = velocity.get(edge.to);
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const distance = Math.max(1, Math.hypot(dx, dy));
        const force = (distance - 260) * 0.0028 * cooling;
        va.x += (dx / distance) * force;
        va.y += (dy / distance) * force;
        vb.x -= (dx / distance) * force;
        vb.y -= (dy / distance) * force;
      }
      for (const id of ids) {
        const position = this.positions.get(id);
        const vector = velocity.get(id);
        vector.x = (vector.x - position.x * 0.0007) * 0.82;
        vector.y = (vector.y - position.y * 0.0007) * 0.82;
        position.x += vector.x;
        position.y += vector.y;
      }
    }
  }

  graphLevels() {
    const visibleEdges = this.filteredEdges(false);
    const outgoing = adjacency(visibleEdges, "from", "to");
    const levels = new Map();
    const queue = [];
    const roots = this.roots.size ? [...this.roots] : this.zeroIndegreeRoots(visibleEdges);
    for (const root of roots) {
      if (!this.nodes.has(root)) continue;
      levels.set(root, 0);
      queue.push(root);
    }
    for (let cursor = 0; cursor < queue.length; cursor += 1) {
      const id = queue[cursor];
      const nextLevel = levels.get(id) + 1;
      for (const next of outgoing.get(id) || []) {
        if (levels.has(next)) continue;
        levels.set(next, nextLevel);
        queue.push(next);
      }
    }
    const fallback = Math.max(0, ...levels.values()) + 1;
    for (const id of this.nodes.keys()) {
      if (!levels.has(id)) levels.set(id, fallback);
    }
    return levels;
  }

  zeroIndegreeRoots(edges) {
    const incoming = new Map([...this.nodes.keys()].map((id) => [id, 0]));
    for (const edge of edges) incoming.set(edge.to, (incoming.get(edge.to) || 0) + 1);
    const roots = [...incoming].filter(([, count]) => count === 0).map(([id]) => id);
    return roots.length ? roots : [...this.nodes.keys()].slice(0, 1);
  }

  renderGraph() {
    this.edgeLayer.replaceChildren();
    this.nodeLayer.replaceChildren();
    this.renderedNodeElements.clear();
    this.renderedEdgesByNode.clear();
    const edges = this.filteredEdges(true);
    const visibleNodes = this.visibleNodeSet(edges);
    this.$('[data-role="empty"]').hidden = visibleNodes.size > 0;
    const keyboardNode = visibleNodes.has(this.selectedId)
      ? this.selectedId
      : [...this.roots].find((id) => visibleNodes.has(id)) || visibleNodes.values().next().value;

    for (const edge of edges) {
      if (!visibleNodes.has(edge.from) || !visibleNodes.has(edge.to)) continue;
      const from = this.positions.get(edge.from);
      const to = this.positions.get(edge.to);
      if (!from || !to) continue;
      const path = svg("path", {
        class: `dg-edge dg-kind-${edge.kind}${edge.optional ? " is-optional" : ""}${
          edge.synthetic ? " is-synthetic" : ""
        }${this.isEdgeFocused(edge) ? " is-focused" : ""}`,
        d: edgePath(from, to),
        "marker-end": "url(#dg-arrow)",
      });
      path.appendChild(
        svg("title", {}, `${nodeLabel(this.nodes.get(edge.from))} → ${nodeLabel(this.nodes.get(edge.to))} · ${edge.kind}`)
      );
      this.edgeLayer.appendChild(path);
      const renderedEdge = { edge, path };
      for (const nodeId of [edge.from, edge.to]) {
        if (!this.renderedEdgesByNode.has(nodeId)) this.renderedEdgesByNode.set(nodeId, []);
        this.renderedEdgesByNode.get(nodeId).push(renderedEdge);
      }
    }

    const matches = this.searchMatches();
    for (const id of visibleNodes) {
      const node = this.nodes.get(id);
      const position = this.positions.get(id);
      if (!position) continue;
      const selected = id === this.selectedId;
      const focused = !this.focusNodes || this.focusNodes.has(id);
      const searchMatch = matches.has(id);
      const group = svg("g", {
        class: `dg-node${selected ? " is-selected" : ""}${node.root ? " is-root" : ""}${
          node.synthetic ? " is-synthetic" : ""
        }${focused ? " is-focused" : " is-dimmed"}${searchMatch ? " is-search-match" : ""}`,
        transform: `translate(${position.x - NODE_WIDTH / 2} ${position.y - NODE_HEIGHT / 2})`,
        tabindex: id === keyboardNode ? "0" : "-1",
        role: "button",
        "aria-label": `${nodeLabel(node)}${node.version ? ` version ${node.version}` : ""}`,
        "data-node-id": id,
      });
      group.appendChild(svg("rect", { width: NODE_WIDTH, height: NODE_HEIGHT, rx: 14 }));
      group.appendChild(svg("circle", { class: "dg-node-dot", cx: 19, cy: 21, r: 5 }));
      group.appendChild(svg("text", { class: "dg-node-title", x: 32, y: 25 }, truncate(nodeLabel(node), 27)));
      group.appendChild(
        svg("text", { class: "dg-node-meta", x: 18, y: 47 }, truncate(nodeMeta(node), 34))
      );
      if (this.roots.has(id)) {
        group.appendChild(svg("text", { class: "dg-root-label", x: NODE_WIDTH - 12, y: 17, "text-anchor": "end" }, "ROOT"));
      }
      group.appendChild(svg("title", {}, `${nodeLabel(node)}\n${nodeMeta(node)}`));
      group.addEventListener("click", (event) => {
        if (!this.drag?.moved) this.selectNode(id, true);
        event.stopPropagation();
      });
      group.addEventListener("dblclick", (event) => {
        event.preventDefault();
        this.navigateToNode(node);
      });
      group.addEventListener("keydown", (event) => {
        if (event.key === "Enter" && event.shiftKey) this.navigateToNode(node);
        else if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          this.selectNode(id, true);
        } else if (["ArrowRight", "ArrowDown"].includes(event.key)) {
          event.preventDefault();
          this.focusAdjacentNode(id, 1);
        } else if (["ArrowLeft", "ArrowUp"].includes(event.key)) {
          event.preventDefault();
          this.focusAdjacentNode(id, -1);
        } else if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          this.focusBoundaryNode(event.key === "Home");
        }
      });
      group.addEventListener("pointerdown", (event) => this.onNodePointerDown(event, id));
      this.nodeLayer.appendChild(group);
      this.renderedNodeElements.set(id, group);
    }
    this.updateTransform();
    this.renderInspector();
    this.updateMetrics();
  }

  filteredEdges(applyFocus) {
    return this.edges.filter((edge) => {
      if (!this.enabledKinds.has(edge.kind)) return false;
      if (!this.includeOptional && edge.optional) return false;
      if (applyFocus && this.focusEdges && !this.focusEdges.has(edgePairIdentity(edge.from, edge.to))) {
        return false;
      }
      if (applyFocus && this.focusNodes && !(this.focusNodes.has(edge.from) && this.focusNodes.has(edge.to))) {
        return false;
      }
      return true;
    });
  }

  visibleNodeSet(edges) {
    const visible = new Set();
    for (const edge of edges) {
      visible.add(edge.from);
      visible.add(edge.to);
    }
    for (const root of this.roots) visible.add(root);
    if (this.focusNodes) {
      for (const id of this.focusNodes) {
        if (this.nodes.has(id)) visible.add(id);
      }
      for (const id of [...visible]) if (!this.focusNodes.has(id)) visible.delete(id);
    }
    if (!edges.length && !this.focusNodes) {
      for (const id of this.nodes.keys()) visible.add(id);
    }
    return visible;
  }

  searchMatches() {
    const matches = new Set();
    if (!this.searchTerm) return matches;
    for (const [id, node] of this.nodes) {
      const haystack = `${node.org}/${node.name} ${node.version} ${[...node.requirements].join(" ")}`.toLowerCase();
      if (haystack.includes(this.searchTerm)) matches.add(id);
    }
    return matches;
  }

  isEdgeFocused(edge) {
    return (
      edge.from === this.selectedId ||
      edge.to === this.selectedId ||
      (this.focusNodes && this.focusNodes.has(edge.from) && this.focusNodes.has(edge.to))
    );
  }

  selectNode(id, updateHash) {
    if (id === null) {
      this.selectedId = null;
      this.renderGraph();
      return;
    }
    if (!this.nodes.has(id)) return;
    this.selectedId = id;
    if (updateHash) {
      const node = this.nodes.get(id);
      history.replaceState(null, "", `#dependency-graph=${encodeURIComponent(`${node.org}/${node.name}`)}`);
    }
    this.renderGraph();
    if (updateHash) {
      requestAnimationFrame(() => {
        const selected = [...this.nodeLayer.querySelectorAll("[data-node-id]")].find(
          (element) => element.dataset.nodeId === id
        );
        selected?.focus({ preventScroll: true });
      });
    }
  }

  focusAdjacentNode(id, direction) {
    const ids = [...this.renderedNodeElements.keys()];
    if (ids.length < 2) return;
    const current = Math.max(0, ids.indexOf(id));
    const next = (current + direction + ids.length) % ids.length;
    this.selectNode(ids[next], true);
  }

  focusBoundaryNode(first) {
    const ids = [...this.renderedNodeElements.keys()];
    if (ids.length) this.selectNode(first ? ids[0] : ids[ids.length - 1], true);
  }

  renderInspector() {
    const node = this.nodes.get(this.selectedId);
    if (!node) {
      this.inspector.innerHTML = `<div class="dg-inspector-empty">
        <div class="dg-orbit" aria-hidden="true"></div>
        <h3>Select a package</h3>
        <p>Inspect identity, requirements, features, incoming impact, and outgoing dependencies.</p>
      </div>`;
      return;
    }
    const outgoing = this.edges.filter((edge) => edge.from === node.id);
    const incoming = this.edges.filter((edge) => edge.to === node.id);
    const requirements = [...node.requirements];
    const features = [...node.features];
    const expandable = !node.resolved && !node.expanded;
    this.inspector.innerHTML = `
      <div class="dg-inspector-head">
        <p class="dg-eyebrow">Selected package</p>
        <h3>${escapeHtml(node.org)}/${escapeHtml(node.name)}</h3>
        <p class="dg-identity">${escapeHtml(node.registryId)}</p>
      </div>
      <dl class="dg-detail-grid">
        <dt>Version</dt><dd>${escapeHtml(node.version || "not resolved")}</dd>
        <dt>Requirements</dt><dd>${requirements.length ? requirements.map(escapeHtml).join(", ") : "—"}</dd>
        <dt>Dependencies</dt><dd>${outgoing.length}</dd>
        <dt>Dependents</dt><dd>${incoming.length}</dd>
        <dt>Features</dt><dd>${features.length ? features.map(escapeHtml).join(", ") : "—"}</dd>
        <dt>Artifact</dt><dd class="dg-digest">${escapeHtml(shortDigest(node.artifactDigest) || "—")}</dd>
      </dl>
      <div class="dg-inspector-actions">
        <a class="button" href="${packagePageUrl(node.org, node.name)}">Open package</a>
        ${
          expandable
            ? `<button class="button primary" type="button" data-inspector-action="expand">Expand latest declared graph</button>`
            : ""
        }
      </div>
      ${
        node.synthetic
          ? `<p class="dg-caveat">This node was expanded using its latest declared manifest. It is navigation context, not an exact lockfile resolution.</p>`
          : ""
      }
      <div class="dg-neighbor-list">
        <h4>Outgoing relationships</h4>
        ${
          outgoing.length
            ? outgoing
                .slice(0, 18)
                .map(
                  (edge) => `<button type="button" data-select-node="${escapeHtml(edge.to)}">
                    <span>${escapeHtml(nodeLabel(this.nodes.get(edge.to)))}</span>
                    <small>${escapeHtml(edge.kind)}${edge.requirement ? ` · ${escapeHtml(edge.requirement)}` : ""}</small>
                  </button>`
                )
                .join("")
            : `<p>No outgoing relationships in the loaded graph.</p>`
        }
      </div>`;

    this.inspector.querySelector('[data-inspector-action="expand"]')?.addEventListener("click", () =>
      this.expandLatest(node)
    );
    this.inspector.querySelectorAll('[data-select-node]').forEach((button) =>
      button.addEventListener("click", () => this.selectNode(button.dataset.selectNode, true))
    );
  }

  async expandLatest(node) {
    node.expanded = true;
    this.setStatus(`Expanding latest declared graph for ${node.org}/${node.name}…`, "loading");
    try {
      const result = await this.fetchDocument(latestDocumentUrl(node.org, node.name));
      this.addDocument(result.document, { synthetic: true });
      node.version ||= result.selectedVersion;
      node.synthetic = true;
      this.syntheticTraversal = true;
      this.showNotice(
        "Expanded nodes use each package’s latest declared manifest. This visual traversal does not replace an exact zpkg lock/resolution graph.",
        "info"
      );
      this.applyLayout(false);
      this.renderAccessibleTable();
      this.setStatus(`Expanded ${node.org}/${node.name}@${result.selectedVersion || "latest"}.`, "ready");
    } catch (error) {
      node.expanded = false;
      this.fail(error);
    }
  }

  runQuery(query) {
    if (query === "clear") {
      this.focusNodes = null;
      this.focusEdges = null;
      this.focusLabel = "";
      this.setStatus("Query focus cleared.", "ready");
      this.renderGraph();
      return;
    }
    const selected = this.selectedId || [...this.roots][0];
    if (!selected && query !== "cycles") {
      this.setStatus("Select a package before running this query.", "error");
      return;
    }

    if (query === "pin-path") {
      this.pathStartId = selected;
      this.setStatus(`Path start pinned at ${nodeLabel(this.nodes.get(selected))}. Select an endpoint and run Shortest path.`, "ready");
      return;
    }

    const queryEdges = this.filteredEdges(false);
    const outgoing = adjacency(queryEdges, "from", "to");
    const incoming = adjacency(queryEdges, "to", "from");
    let result = null;
    let label = "";
    const previousFocusEdges = this.focusEdges;
    this.focusEdges = null;
    if (query === "direct") {
      result = new Set([selected, ...(outgoing.get(selected) || [])]);
      this.focusEdges = new Set(
        queryEdges
          .filter((edge) => edge.from === selected)
          .map((edge) => edgePairIdentity(edge.from, edge.to))
      );
      label = `Direct dependencies of ${nodeLabel(this.nodes.get(selected))}`;
    } else if (query === "transitive") {
      result = this.walk(selected, outgoing);
      label = `Transitive dependencies of ${nodeLabel(this.nodes.get(selected))}`;
    } else if (query === "dependents") {
      result = this.walk(selected, incoming);
      label = `Reverse impact of ${nodeLabel(this.nodes.get(selected))}`;
    } else if (query === "cycles") {
      result = this.cycleNodes(outgoing, incoming);
      label = result.size ? "Packages participating in dependency cycles" : "No cycles detected";
    } else if (query === "longest") {
      const { path, cyclic } = this.longestChain(selected, outgoing);
      if (cyclic) {
        this.focusEdges = previousFocusEdges;
        this.setStatus(
          "An exact longest chain is undefined for a graph with reachable cycles. Run Cycles or filter those edges first.",
          "error"
        );
        return;
      }
      result = new Set(path);
      this.focusEdges = pathEdgePairs(path);
      label = path.length > 1 ? `Longest loaded chain (${path.length - 1} edges)` : "No outgoing chain found";
    } else if (query === "shortest") {
      if (!this.pathStartId) {
        this.focusEdges = previousFocusEdges;
        this.setStatus("Pin a path start first, then select the endpoint.", "error");
        return;
      }
      const path = this.shortestPath(this.pathStartId, selected, outgoing);
      result = new Set(path);
      this.focusEdges = pathEdgePairs(path);
      label = path.length ? `Shortest directed path (${path.length - 1} edges)` : "No directed path found";
    }
    if (!result) return;
    this.focusNodes = result;
    this.focusLabel = label;
    this.setStatus(`${label}: ${result.size} package(s).`, result.size ? "ready" : "error");
    this.renderGraph();
    requestAnimationFrame(() => this.fitGraph());
  }

  walk(start, neighbors) {
    const visited = new Set([start]);
    const queue = [start];
    for (let cursor = 0; cursor < queue.length; cursor += 1) {
      const current = queue[cursor];
      for (const next of neighbors.get(current) || []) {
        if (visited.has(next)) continue;
        visited.add(next);
        queue.push(next);
      }
    }
    return visited;
  }

  shortestPath(start, end, outgoing) {
    if (start === end) return [start];
    const queue = [start];
    const previous = new Map([[start, null]]);
    for (let cursorIndex = 0; cursorIndex < queue.length; cursorIndex += 1) {
      const current = queue[cursorIndex];
      for (const next of outgoing.get(current) || []) {
        if (previous.has(next)) continue;
        previous.set(next, current);
        if (next === end) {
          const path = [end];
          let cursor = current;
          while (cursor) {
            path.push(cursor);
            cursor = previous.get(cursor);
          }
          return path.reverse();
        }
        queue.push(next);
      }
    }
    return [];
  }

  longestChain(start, outgoing) {
    const reachable = this.walk(start, outgoing);
    const indegree = new Map([...reachable].map((id) => [id, 0]));
    for (const id of reachable) {
      for (const next of outgoing.get(id) || []) {
        if (reachable.has(next)) indegree.set(next, indegree.get(next) + 1);
      }
    }

    const queue = [...indegree].filter(([, count]) => count === 0).map(([id]) => id);
    const ordered = [];
    for (let cursor = 0; cursor < queue.length; cursor += 1) {
      const id = queue[cursor];
      ordered.push(id);
      for (const next of outgoing.get(id) || []) {
        if (!indegree.has(next)) continue;
        const remaining = indegree.get(next) - 1;
        indegree.set(next, remaining);
        if (remaining === 0) queue.push(next);
      }
    }
    if (ordered.length !== reachable.size) return { path: [], cyclic: true };

    const distance = new Map([[start, 0]]);
    const previous = new Map();
    for (const id of ordered) {
      const currentDistance = distance.get(id);
      if (currentDistance === undefined) continue;
      for (const next of outgoing.get(id) || []) {
        if (!reachable.has(next)) continue;
        const candidate = currentDistance + 1;
        if (candidate > (distance.get(next) ?? -1)) {
          distance.set(next, candidate);
          previous.set(next, id);
        }
      }
    }

    let end = start;
    for (const [id, value] of distance) {
      if (value > (distance.get(end) ?? -1)) end = id;
    }
    const path = [end];
    while (previous.has(path[path.length - 1])) {
      path.push(previous.get(path[path.length - 1]));
    }
    path.reverse();
    return { path, cyclic: false };
  }

  cycleNodes(outgoing, incoming) {
    const visited = new Set();
    const finishOrder = [];
    const cycles = new Set();

    for (const start of this.nodes.keys()) {
      if (visited.has(start)) continue;
      const stack = [[start, false]];
      while (stack.length) {
        const [id, expanded] = stack.pop();
        if (expanded) {
          finishOrder.push(id);
          continue;
        }
        if (visited.has(id)) continue;
        visited.add(id);
        stack.push([id, true]);
        const nextNodes = [...(outgoing.get(id) || [])];
        for (let index = nextNodes.length - 1; index >= 0; index -= 1) {
          if (!visited.has(nextNodes[index])) stack.push([nextNodes[index], false]);
        }
      }
    }

    const assigned = new Set();
    for (let orderIndex = finishOrder.length - 1; orderIndex >= 0; orderIndex -= 1) {
      const start = finishOrder[orderIndex];
      if (assigned.has(start)) continue;
      const component = [];
      const stack = [start];
      assigned.add(start);
      while (stack.length) {
        const id = stack.pop();
        component.push(id);
        for (const next of incoming.get(id) || []) {
          if (assigned.has(next)) continue;
          assigned.add(next);
          stack.push(next);
        }
      }
      const selfLoop = component.length === 1 && (outgoing.get(start) || new Set()).has(start);
      if (component.length > 1 || selfLoop) component.forEach((id) => cycles.add(id));
    }
    return cycles;
  }

  resetView() {
    this.focusNodes = null;
    this.focusEdges = null;
    this.focusLabel = "";
    this.pathStartId = null;
    this.searchTerm = "";
    this.$('[data-control="search"]').value = "";
    this.enabledKinds = new Set(Object.keys(KIND_LABELS));
    this.$$('[data-kind]').forEach((checkbox) => (checkbox.checked = true));
    this.includeOptional = true;
    this.$('[data-control="optional"]').checked = true;
    this.renderGraph();
    this.fitGraph();
    this.setStatus("Graph view reset.", "ready");
  }

  updateMetrics() {
    const levels = this.graphLevels();
    const depth = levels.size ? Math.max(...levels.values()) : 0;
    this.$('[data-metric="nodes"]').textContent = String(this.nodes.size);
    this.$('[data-metric="edges"]').textContent = String(this.edges.length);
    this.$('[data-metric="depth"]').textContent = String(depth);
  }

  renderAccessibleTable() {
    const rows = this.edges
      .map((edge) => {
        const from = this.nodes.get(edge.from);
        const to = this.nodes.get(edge.to);
        return `<tr>
          <td><a href="${packagePageUrl(from.org, from.name)}">${escapeHtml(nodeLabel(from))}</a></td>
          <td><a href="${packagePageUrl(to.org, to.name)}">${escapeHtml(nodeLabel(to))}</a></td>
          <td>${escapeHtml(edge.kind)}</td>
          <td>${escapeHtml(edge.requirement || "—")}</td>
          <td>${edge.optional ? "yes" : "no"}</td>
        </tr>`;
      })
      .join("");
    this.$('[data-role="table"]').innerHTML = this.edges.length
      ? `<table><caption>Loaded dependency relationships</caption><thead><tr><th scope="col">From</th><th scope="col">To</th><th scope="col">Kind</th><th scope="col">Requirement</th><th scope="col">Optional</th></tr></thead><tbody>${rows}</tbody></table>`
      : `<p>No dependency relationships are loaded.</p>`;
  }

  updateExportLinks() {
    if (this.mode !== "package") return;
    const version = this.dataset.version || this.versions[0]?.version || "";
    this.$$('[data-export]').forEach((link) => {
      link.href = packageExportUrl(this.dataset.org, this.dataset.package, version, link.dataset.export);
      link.download = `${safeFilename(this.dataset.org)}_${safeFilename(this.dataset.package)}_${safeFilename(version)}.dependency-graph.${extensionFor(link.dataset.export)}`;
    });
  }

  fitGraph() {
    const visible = [...this.nodeLayer.querySelectorAll(".dg-node:not(.is-dimmed)")];
    if (!visible.length) return;
    const bounds = graphBounds(
      visible.map((element) => this.positions.get(element.dataset.nodeId)).filter(Boolean)
    );
    const width = Math.max(1, this.viewport.clientWidth);
    const height = Math.max(1, this.viewport.clientHeight);
    const graphWidth = Math.max(1, bounds.maxX - bounds.minX + NODE_WIDTH + 120);
    const graphHeight = Math.max(1, bounds.maxY - bounds.minY + NODE_HEIGHT + 120);
    const k = clamp(Math.min(width / graphWidth, height / graphHeight), 0.14, 1.35);
    const centerX = (bounds.minX + bounds.maxX) / 2;
    const centerY = (bounds.minY + bounds.maxY) / 2;
    this.transform = { x: width / 2 - centerX * k, y: height / 2 - centerY * k, k };
    this.updateTransform();
  }

  updateTransform() {
    const { x, y, k } = this.transform;
    this.world.setAttribute("transform", `translate(${x} ${y}) scale(${k})`);
    this.$('[data-role="zoom"]').textContent = `${Math.round(k * 100)}%`;
  }

  onWheel(event) {
    event.preventDefault();
    this.zoomAt(Math.exp(-event.deltaY * 0.0013), event.offsetX, event.offsetY);
  }

  zoomAt(factor, screenX, screenY) {
    const old = this.transform;
    const nextK = clamp(old.k * factor, 0.08, 4);
    const worldX = (screenX - old.x) / old.k;
    const worldY = (screenY - old.y) / old.k;
    this.transform = {
      x: screenX - worldX * nextK,
      y: screenY - worldY * nextK,
      k: nextK,
    };
    this.updateTransform();
  }

  onCanvasPointerDown(event) {
    if (event.button !== 0 || event.target.closest(".dg-node")) return;
    this.svg.setPointerCapture?.(event.pointerId);
    this.drag = {
      type: "pan",
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      originX: this.transform.x,
      originY: this.transform.y,
      moved: false,
    };
  }

  onNodePointerDown(event, id) {
    if (event.button !== 0) return;
    event.stopPropagation();
    const position = this.positions.get(id);
    this.drag = {
      type: "node",
      pointerId: event.pointerId,
      id,
      startX: event.clientX,
      startY: event.clientY,
      originX: position.x,
      originY: position.y,
      moved: false,
    };
  }

  onPointerMove(event) {
    if (!this.drag || event.pointerId !== this.drag.pointerId) return;
    const dx = event.clientX - this.drag.startX;
    const dy = event.clientY - this.drag.startY;
    this.drag.moved ||= Math.abs(dx) + Math.abs(dy) > 4;
    if (this.drag.type === "pan") {
      this.transform.x = this.drag.originX + dx;
      this.transform.y = this.drag.originY + dy;
      this.updateTransform();
    } else {
      const position = this.positions.get(this.drag.id);
      position.x = this.drag.originX + dx / this.transform.k;
      position.y = this.drag.originY + dy / this.transform.k;
      this.updateDraggedNode(this.drag.id);
    }
  }

  updateDraggedNode(id) {
    const position = this.positions.get(id);
    const element = this.renderedNodeElements.get(id);
    if (!position || !element) return;
    element.setAttribute(
      "transform",
      `translate(${position.x - NODE_WIDTH / 2} ${position.y - NODE_HEIGHT / 2})`
    );
    for (const { edge, path } of this.renderedEdgesByNode.get(id) || []) {
      const from = this.positions.get(edge.from);
      const to = this.positions.get(edge.to);
      if (from && to) path.setAttribute("d", edgePath(from, to));
    }
  }

  onPointerUp(event) {
    if (!this.drag || event.pointerId !== this.drag.pointerId) return;
    const moved = this.drag.moved;
    const type = this.drag.type;
    this.drag = { moved };
    queueMicrotask(() => {
      if (this.drag?.moved === moved && this.drag?.type === undefined) this.drag = null;
    });
    if (type === "pan" && !moved) this.selectNode(null, false);
  }

  navigateToNode(node) {
    location.href = packagePageUrl(node.org, node.name);
  }

  setStatus(message, state) {
    this.status.textContent = message;
    this.status.dataset.state = state;
  }

  showNotice(message, type) {
    this.notice.hidden = false;
    this.notice.dataset.type = type;
    this.notice.textContent = message;
  }

  fail(error) {
    console.error(error);
    this.setStatus(error?.message || "The dependency graph could not be loaded.", "error");
    this.renderGraph();
  }
}

function packageDocumentUrl(org, name, version) {
  return `/bff/dependency-graphs/packages/${segment(org)}/${segment(name)}/${segment(version)}`;
}

function latestDocumentUrl(org, name) {
  return `/bff/dependency-graphs/packages/${segment(org)}/${segment(name)}/latest`;
}

function packageExportUrl(org, name, version, format) {
  return `${packageDocumentUrl(org, name, version)}/export/${segment(format)}`;
}

function packagePageUrl(org, name) {
  return `/p/${segment(org)}/${segment(name)}#dependency-graph`;
}

function segment(value) {
  return encodeURIComponent(String(value));
}

function coordinateKey(identity) {
  return `${identity.registry_id || "registry:unknown"}::${identity.org}/${identity.name}`;
}

function resolvedKey(identity) {
  return `${coordinateKey(identity)}@${identity.version}`;
}

function nodeLabel(node) {
  return node ? `${node.org}/${node.name}` : "unknown package";
}

function nodeMeta(node) {
  if (node.version) return `v${node.version}${node.synthetic ? " · latest expansion" : ""}`;
  const requirement = [...node.requirements][0];
  return requirement ? `requires ${requirement}` : "version unresolved";
}

function edgePath(from, to) {
  const direction = to.x >= from.x ? 1 : -1;
  const startX = from.x + (NODE_WIDTH / 2 + 8) * direction;
  const endX = to.x - (NODE_WIDTH / 2 + 8) * direction;
  const span = Math.max(70, Math.abs(endX - startX) * 0.48);
  const control1 = startX + span * direction;
  const control2 = endX - span * direction;
  return `M ${startX} ${from.y} C ${control1} ${from.y}, ${control2} ${to.y}, ${endX} ${to.y}`;
}

function adjacency(edges, fromKey, toKey) {
  const map = new Map();
  for (const edge of edges) {
    if (!map.has(edge[fromKey])) map.set(edge[fromKey], new Set());
    map.get(edge[fromKey]).add(edge[toKey]);
  }
  return map;
}

function edgeIdentity(edge) {
  return [edge.from, edge.to, edge.kind, edge.requirement, edge.target, edge.optional].join("\u0000");
}

function edgePairIdentity(from, to) {
  return `${from}\u0000${to}`;
}

function pathEdgePairs(path) {
  const pairs = new Set();
  for (let index = 1; index < path.length; index += 1) {
    pairs.add(edgePairIdentity(path[index - 1], path[index]));
  }
  return pairs;
}

function graphBounds(positions) {
  const xs = positions.map((position) => position.x);
  const ys = positions.map((position) => position.y);
  return {
    minX: Math.min(...xs),
    maxX: Math.max(...xs),
    minY: Math.min(...ys),
    maxY: Math.max(...ys),
  };
}

function svg(tag, attributes = {}, text = null) {
  const element = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attributes)) element.setAttribute(name, value);
  if (text !== null) element.textContent = text;
  return element;
}

async function mapLimit(values, limit, worker) {
  let cursor = 0;
  const runners = Array.from({ length: Math.min(limit, values.length) }, async () => {
    while (cursor < values.length) {
      const index = cursor++;
      await worker(values[index], index);
    }
  });
  await Promise.all(runners);
}

function scopeSourceBatches(sources) {
  const indexed = sources.map((source, index) => ({ source, index }));
  return {
    publicSources: indexed.filter(({ source }) => !source.private),
    privateSources: indexed.filter(({ source }) => source.private),
  };
}

function parseJson(value, fallback) {
  try {
    return value ? JSON.parse(value) : fallback;
  } catch (error) {
    console.warn("Invalid dependency graph component data", error);
    return fallback;
  }
}

function extensionFor(format) {
  return {
    yaml: "yaml",
    toml: "toml",
    json5: "json5",
    xml: "xml",
    csv: "csv",
    msgpack: "msgpack",
    protobuf: "pb",
    dot: "dot",
    mermaid: "mmd",
  }[format] || "json";
}

function safeFilename(value) {
  return String(value || "graph").replace(/[^A-Za-z0-9.+-]/g, "_");
}

function shortDigest(value) {
  if (!value) return "";
  return value.length > 24 ? `${value.slice(0, 21)}…` : value;
}

function isStrongGraphEtag(value) {
  // RFC 9110 strong entity-tag: quoted opaque tag, without the W/ prefix.
  // The graph digest has a separate canonical sha256 shape; an ETag need not.
  return /^"(?:[\x21\x23-\x7e]|[\u0080-\u00ff])*"$/.test(value);
}

function isGraphDigest(value) {
  return /^sha256:[0-9a-f]{64}$/.test(value);
}

function truncate(value, length) {
  const text = String(value || "");
  return text.length > length ? `${text.slice(0, length - 1)}…` : text;
}

function capitalize(value) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function safeStorageGet(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function safeStorageSet(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage is optional; graph behavior remains deterministic without it.
  }
}

if (globalThis.customElements && !customElements.get("zed-dependency-graph")) {
  customElements.define("zed-dependency-graph", ZedDependencyGraph);
}

export {
  ZedDependencyGraph,
  adjacency,
  edgePairIdentity,
  isGraphDigest,
  isStrongGraphEtag,
  packageDocumentUrl,
  packageExportUrl,
  packagePageUrl,
  pathEdgePairs,
  scopeSourceBatches,
};
