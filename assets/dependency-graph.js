const SVG_NS = "http://www.w3.org/2000/svg";
const NODE_WIDTH = 224;
const NODE_HEIGHT = 64;
const SCOPE_LIMIT = 80;
const SCOPE_BATCH_SIZE = 4;
const FORCE_LIMIT = 260;
const MAX_GRAPH_NODES = 3000;
const MAX_GRAPH_EDGES = 12000;
const MAX_RENDERED_NODES = 750;
const MAX_RENDERED_EDGES = 2500;
const MAX_ACCESSIBLE_EDGES = 2000;
const MAX_FRAGMENT_TABLE_EDGES = 250;
const MAX_PROJECTION_DIMENSION = 4096;
const MAX_GRAPH_DOCUMENT_BYTES = 32 * 1024 * 1024;
const MAX_DOCUMENT_CACHE_ENTRIES = 4;
const FETCH_TIMEOUT_MS = 12000;
const MAX_GRAPH_TEXT_BYTES = 2048;
const MAX_EDGE_FEATURES = 256;
const MAX_VIEW_STATE_TEXT_LENGTH = 1024;
const GRAPH_TEXT_ENCODER = new TextEncoder();
const QUERY_RESULT_PAGE_SIZE = 25;
const UNMAINTAINED_AFTER_DAYS = 365;
const MILLISECONDS_PER_DAY = 24 * 60 * 60 * 1000;
const HTMLElementBase = globalThis.HTMLElement || class {};
let graphInstanceSequence = 0;

function graphInstanceIdentifiers() {
  const namespace = `dg-${(++graphInstanceSequence).toString(36)}`;
  return Object.freeze({
    namespace,
    keyboardInstructions: `${namespace}-keyboard-instructions`,
    svgTitle: `${namespace}-svg-title`,
    svgDescription: `${namespace}-svg-description`,
    arrow: `${namespace}-arrow`,
  });
}

const KIND_LABELS = {
  runtime: "Runtime",
  build: "Build",
  development: "Development",
  peer: "Peer",
  tooling: "Tooling",
};

const GRAPH_LAYOUTS = new Set(["layered", "radial", "force"]);
const GRAPH_CHANNELS = new Set(["all", "stable", "prerelease"]);
const GRAPH_QUERIES = new Set([
  "direct",
  "transitive",
  "dependents",
  "cycles",
  "longest",
  "shortest",
  "internal",
  "external",
  "duplicates",
  "prerelease",
  "yanked",
  "centrality",
  "licenses",
  "license-review",
  "unmaintained",
]);
const GRAPH_STATE_PARAMETERS = Object.freeze({
  layout: "graph-layout",
  search: "graph-search",
  kinds: "graph-kinds",
  optional: "graph-optional",
  query: "graph-query",
  queryAnchor: "graph-query-node",
  selected: "graph-node",
  pathStart: "graph-path-start",
  channel: "graph-channel",
  version: "graph-version",
});

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
    this.identifiers = graphInstanceIdentifiers();
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
    this.activeQuery = "";
    this.queryAnchorId = "";
    this.queryPage = 0;
    this.searchTerm = "";
    this.searchValue = "";
    this.layoutName = "layered";
    this.channel = "all";
    this.enabledKinds = new Set(Object.keys(KIND_LABELS));
    this.includeOptional = true;
    this.pendingSelectedId = "";
    this.pendingQueryAnchorId = "";
    this.pendingPathStartId = "";
    this.transform = { x: 0, y: 0, k: 1 };
    this.drag = null;
    this.cache = new Map();
    this.serialFetchTail = Promise.resolve();
    this.fetchControllers = new Set();
    this.fetchGeneration = 0;
    this.loadSequence = 0;
    this.sourceFailures = 0;
    this.syntheticTraversal = false;
    this.loadStartedAt = 0;
    this.resizeObserver = null;
    this.handleWindowPointerMove = (event) => this.onPointerMove(event);
    this.handleWindowPointerUp = (event) => this.onPointerUp(event);
    this.handleHashChange = () => this.restoreSelectionFromHash();
    this.handleLocationChange = () => this.restoreViewFromLocation();
    this.searchFrame = null;
    this.fragmentSequences = new Map();
  }

  connectedCallback() {
    if (this.dataset.ready === "true") return;
    this.claimElementId();
    this.setAttribute("data-graph-namespace", this.identifiers.namespace);
    this.dataset.ready = "true";
    this.mode = this.dataset.mode || "package";
    this.versions = parseJson(this.dataset.versions, []);
    this.sources = parseJson(this.dataset.sources, []);
    this.defaultVersion = this.dataset.version || this.versions[0]?.version || "";
    const storedLayout = safeStorageGet("zpkg.graph.layout") || "layered";
    const initialState = parseGraphViewState(globalThis.location?.href, {
      layout: GRAPH_LAYOUTS.has(storedLayout) ? storedLayout : "layered",
    });
    this.applyParsedViewState(initialState);
    this.renderShell();
    this.bindControls();
    this.loadInitial();
  }

  disconnectedCallback() {
    this.loadSequence += 1;
    this.fetchGeneration += 1;
    for (const controller of this.fetchControllers) controller.abort();
    this.fetchControllers.clear();
    this.cache.clear();
    window.removeEventListener("pointermove", this.handleWindowPointerMove);
    window.removeEventListener("pointerup", this.handleWindowPointerUp);
    window.removeEventListener("hashchange", this.handleHashChange);
    window.removeEventListener("popstate", this.handleLocationChange);
    this.resizeObserver?.disconnect();
    for (const target of [this.inspector, this.querySummary, this.stateFragment, this.$?.('[data-role="table"]')]) {
      if (target) globalThis.htmx?.trigger?.(target, "htmx:abort");
    }
    this.fragmentSequences.clear();
    if (this.searchFrame !== null) cancelAnimationFrame(this.searchFrame);
    this.resizeObserver = null;
    this.searchFrame = null;
    delete this.dataset.ready;
  }

  claimElementId() {
    const preferred = this.id || "dependency-graph";
    const graphs = globalThis.document?.querySelectorAll?.("zed-dependency-graph") || [];
    const occupied = Array.from(graphs).some(
      (element) => element !== this && element.id === preferred
    );
    this.id = occupied ? `${preferred}-${this.identifiers.namespace}` : preferred;
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
              : `<label class="dg-field dg-channel-field">
                   <span>Release channel</span>
                   <select data-control="channel">
                     <option value="all"${this.channel === "all" ? " selected" : ""}>All releases</option>
                     <option value="stable"${this.channel === "stable" ? " selected" : ""}>Stable only</option>
                     <option value="prerelease"${this.channel === "prerelease" ? " selected" : ""}>Pre-release only</option>
                   </select>
                 </label>`
          }
          <label class="dg-field dg-search-field">
            <span>Find package</span>
            <input data-control="search" type="search" value="${escapeHtml(this.searchValue)}" placeholder="org/name or version" autocomplete="off">
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
          <button type="button" class="dg-icon-button" data-action="save" title="Save this view in this browser">Save view</button>
          <button type="button" class="dg-icon-button" data-action="restore" title="Restore the saved view">Restore</button>
          <button type="button" class="dg-icon-button" data-action="share" title="Copy a reproducible link">Copy link</button>
          ${this.exportMenu()}
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
          ${
            this.mode === "scope"
              ? `<button type="button" data-query="internal">In-scope packages</button>
                 <button type="button" data-query="external">External dependencies</button>
                 <button type="button" data-query="duplicates">Multiple versions</button>
                 <button type="button" data-query="prerelease">Pre-release exposure</button>
                 <button type="button" data-query="licenses">License distribution</button>
                 <button type="button" data-query="license-review">License review</button>
                 <button type="button" data-query="unmaintained">Unmaintained packages</button>
                 <button type="button" data-query="centrality">High centrality</button>`
              : `<button type="button" data-query="prerelease">Pre-release exposure</button>
                 <button type="button" data-query="yanked">Yanked exposure</button>
                 <button type="button" data-query="duplicates">Multiple versions</button>
                 <button type="button" data-query="centrality">High centrality</button>`
          }
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
        <div class="dg-status" data-role="status" role="status" aria-live="polite">Preparing graph workspace…</div>
        <div class="dg-state-fragment" data-role="state-fragment" aria-live="polite" hidden></div>
        <div class="dg-degradation" data-role="degradation" role="status" hidden></div>
        <section class="dg-query-summary" data-role="query-summary" aria-labelledby="${this.identifiers.namespace}-query-title" hidden></section>

        <div class="dg-stage">
          <div class="dg-viewport" data-role="viewport">
            <p id="${this.identifiers.keyboardInstructions}" class="dg-sr-only">Use arrow keys to move between packages, Enter to select, Shift plus Enter to open a package, plus and minus to zoom, and zero to fit the graph.</p>
            <svg data-role="svg" role="group" aria-labelledby="${this.identifiers.svgTitle} ${this.identifiers.svgDescription}" aria-describedby="${this.identifiers.keyboardInstructions}" tabindex="0">
              <title id="${this.identifiers.svgTitle}">Interactive package dependency graph</title>
              <desc id="${this.identifiers.svgDescription}">Package nodes and directed dependency relationships. A text relationship table follows the canvas.</desc>
              <defs>
                <marker id="${this.identifiers.arrow}" class="dg-arrow-marker" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto" markerUnits="strokeWidth">
                  <path d="M0,0 L9,4.5 L0,9 z"></path>
                </marker>
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
    this.degradation = this.$('[data-role="degradation"]');
    this.querySummary = this.$('[data-role="query-summary"]');
    this.stateFragment = this.$('[data-role="state-fragment"]');
  }

  exportMenu() {
    return `<details class="dg-export-menu">
      <summary>Download</summary>
      <div>
        ${
          this.mode === "package"
            ? EXPORT_FORMATS.map(
                ([format, label]) =>
                  `<a data-export="${format}" href="#" download>${escapeHtml(label)}</a>`
              ).join("")
            : ""
        }
        <button type="button" data-projection-export="svg">SVG visible projection</button>
        <button type="button" data-projection-export="png">PNG visible projection</button>
      </div>
    </details>`;
  }

  requestFragment(name, target, values, afterSwap = null) {
    const htmx = globalThis.htmx;
    if (!target || typeof htmx?.ajax !== "function") return;
    const base = this.dataset.fragmentBase || "/partials/dependency-graph";
    if (!base.startsWith("/") || base.startsWith("//")) return;
    const sequence = (this.fragmentSequences.get(name) || 0) + 1;
    this.fragmentSequences.set(name, sequence);
    htmx.trigger?.(target, "htmx:abort");
    target.hidden = false;
    Promise.resolve(
      htmx.ajax("POST", `${base}/${name}`, {
        target,
        swap: "innerHTML",
        values,
      })
    )
      .then(() => {
        if (this.fragmentSequences.get(name) === sequence) afterSwap?.();
      })
      .catch(() => {
        // The local semantic renderer remains the progressive fallback.
      });
  }

  requestStateFragment(action) {
    if (!globalThis.location) return;
    const url = `${location.pathname}${location.search}${location.hash}`;
    this.requestFragment("state", this.stateFragment, { action, url });
  }

  bindControls() {
    this.$('[data-control="version"]')?.addEventListener("change", (event) => {
      this.dataset.version = event.target.value;
      this.syncViewUrl();
      this.loadPackage(event.target.value);
    });

    this.$('[data-control="channel"]')?.addEventListener("change", (event) => {
      this.channel = event.target.value;
      this.syncViewUrl();
      this.loadScope();
    });

    this.$('[data-control="search"]').addEventListener("input", (event) => {
      this.searchValue = event.target.value.slice(0, MAX_VIEW_STATE_TEXT_LENGTH);
      this.searchTerm = this.searchValue.trim().toLowerCase();
      if (this.searchFrame !== null) cancelAnimationFrame(this.searchFrame);
      this.searchFrame = requestAnimationFrame(() => {
        this.searchFrame = null;
        this.renderGraph();
        this.syncViewUrl();
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
        this.syncViewUrl();
      });
    });

    this.$$('[data-action]').forEach((button) => {
      button.addEventListener("click", async () => {
        if (button.dataset.action === "fit") this.fitGraph();
        if (button.dataset.action === "reset") this.resetView();
        if (button.dataset.action === "save") this.saveView();
        if (button.dataset.action === "restore") this.restoreSavedView();
        if (button.dataset.action === "share") await this.copyShareLink();
      });
    });

    this.$$('[data-query]').forEach((button) => {
      button.addEventListener("click", () => this.runQuery(button.dataset.query));
    });

    this.$$('[data-kind]').forEach((checkbox) => {
      checkbox.addEventListener("change", () => {
        if (checkbox.checked) this.enabledKinds.add(checkbox.dataset.kind);
        else this.enabledKinds.delete(checkbox.dataset.kind);
        if (this.activeQuery) this.runQuery(this.activeQuery, { sync: false, fit: false, restoring: true });
        else this.renderGraph();
        this.syncViewUrl();
      });
    });

    this.$('[data-control="optional"]').addEventListener("change", (event) => {
      this.includeOptional = event.target.checked;
      if (this.activeQuery) this.runQuery(this.activeQuery, { sync: false, fit: false, restoring: true });
      else this.renderGraph();
      this.syncViewUrl();
    });

    this.$$('[data-projection-export]').forEach((button) => {
      button.addEventListener("click", async () => {
        button.disabled = true;
        try {
          await this.exportProjection(button.dataset.projectionExport);
        } catch (error) {
          this.fail(error);
        } finally {
          button.disabled = false;
        }
      });
    });

    this.svg.addEventListener("wheel", (event) => this.onWheel(event), { passive: false });
    this.svg.addEventListener("pointerdown", (event) => this.onCanvasPointerDown(event));
    window.addEventListener("pointermove", this.handleWindowPointerMove);
    window.addEventListener("pointerup", this.handleWindowPointerUp);
    window.addEventListener("hashchange", this.handleHashChange);
    window.addEventListener("popstate", this.handleLocationChange);
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
    this.loadStartedAt = performanceNow();
    this.setStatus(`Loading ${this.dataset.org}/${this.dataset.package}@${version}…`, "loading");
    this.notice.hidden = true;
    try {
      const url = packageDocumentUrl(this.dataset.org, this.dataset.package, version);
      const { document } = await this.fetchDocument(url, {
        serialized: this.dataset.private === "true",
        noStore: this.dataset.private === "true",
      });
      if (sequence !== this.loadSequence) return;
      assertDeclaredDocumentCoordinate(
        document,
        this.dataset.org,
        this.dataset.package,
        version
      );
      this.clearGraph();
      const root = this.addDocument(document, { primary: true, synthetic: false });
      const versionRow = this.versions.find((item) => item.version === version);
      this.applySourceMetadata(root, {
        prerelease: versionRow?.prerelease,
        yanked: versionRow?.yanked,
      });
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
    const sequence = ++this.loadSequence;
    this.loadStartedAt = performanceNow();
    const channelSources = this.sources.filter(
      (source) => source.version && sourceMatchesChannel(source, this.channel)
    );
    const sources = channelSources.slice(0, SCOPE_LIMIT);
    this.clearGraph();
    this.sourceFailures = 0;
    if (!sources.length) {
      this.setStatus(
        this.channel === "all"
          ? "No published package versions are available in this scope."
          : `No ${this.channel === "stable" ? "stable" : "pre-release"} package versions are available in this scope.`,
        "error"
      );
      this.renderGraph();
      return;
    }
    this.setStatus(`Loading ${sources.length} package graphs…`, "loading");
    let completed = 0;
    // Fetch and apply bounded batches in source order. This keeps at most four
    // wire documents resident at once instead of retaining every graph in a
    // large organization before the node/edge cap can reject excess input.
    for (let offset = 0; offset < sources.length; offset += SCOPE_BATCH_SIZE) {
      if (sequence !== this.loadSequence) return;
      const batch = sources.slice(offset, offset + SCOPE_BATCH_SIZE);
      const loaded = new Array(batch.length);
      const { publicSources, privateSources } = scopeSourceBatches(batch);
      const loadSource = async ({ source, index }) => {
        if (sequence !== this.loadSequence) return;
        try {
          const url = packageDocumentUrl(source.org, source.name, source.version);
          const { document } = await this.fetchDocument(url, {
            serialized: Boolean(source.private),
            noStore: Boolean(source.private),
          });
          assertDeclaredDocumentCoordinate(
            document,
            source.org,
            source.name,
            source.version
          );
          if (sequence !== this.loadSequence) return;
          loaded[index] = document;
        } catch (error) {
          if (sequence !== this.loadSequence) return;
          this.sourceFailures += 1;
          console.warn("Dependency graph source failed", source, error);
        }
        completed += 1;
        this.setStatus(
          `Loaded ${completed} of ${sources.length} package graphs…`,
          "loading"
        );
      };
      // Private graph reads rotate the opaque browser refresh handle. Keep
      // those requests serial while public anonymous reads use bounded fanout.
      await Promise.all([
        mapLimit(publicSources, 3, loadSource),
        mapLimit(privateSources, 1, loadSource),
      ]);
      if (sequence !== this.loadSequence) return;
      loaded.forEach((document, index) => {
        const source = batch[index];
        if (!document) return;
        try {
          const root = this.addDocument(document, {
            primary: offset + index === 0,
            synthetic: false,
            scopeRoot: true,
          });
          this.applySourceMetadata(root, source);
        } catch (error) {
          this.sourceFailures += 1;
          console.warn("Dependency graph source exceeded workspace limits", source, error);
        } finally {
          // The normalized graph model owns the fields the canvas needs. Do
          // not also retain every full scope document in the component cache.
          this.cache.delete(packageDocumentUrl(source.org, source.name, source.version));
        }
      });
    }
    const clipped = channelSources.length > SCOPE_LIMIT;
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

  applySourceMetadata(node, source = {}) {
    if (!node) return;
    node.prerelease =
      typeof source.prerelease === "boolean"
        ? source.prerelease
        : isPrereleaseVersion(node.version);
    node.yanked = Boolean(source.yanked);
    node.private = Boolean(source.private);
    node.license = typeof source.license === "string" ? source.license : "";
    node.updatedAt = typeof source.updatedAt === "string" ? source.updatedAt : "";
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
    this.assertDocumentShape(document);
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
      return root;
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
      return this.nodes.get([...this.roots][0]) || null;
    }
    throw new Error("The server returned an unknown dependency graph view.");
  }

  assertDocumentShape(document) {
    if (document.view === "declared") {
      assertIdentity(document.package, true);
      if (document.dependencies !== undefined && !Array.isArray(document.dependencies)) {
        throw new Error("The declared dependency graph has an invalid dependency list.");
      }
      for (const dependency of document.dependencies || []) {
        assertIdentity(dependency, false);
        assertGraphText(dependency.requirement, "dependency requirement");
        assertOptionalGraphText(dependency.target, "dependency target");
        assertDependencyKind(dependency.kind);
        assertOptionalBoolean(dependency.optional, "dependency optional marker");
        assertStringList(dependency.features, "dependency features");
      }
      return;
    }
    if (document.view === "resolved") {
      if (!Array.isArray(document.nodes) || !Array.isArray(document.edges) || !Array.isArray(document.roots)) {
        throw new Error("The resolved dependency graph has invalid node or edge lists.");
      }
      const nodeIds = new Set();
      for (const node of document.nodes) {
        assertIdentity(node?.id, true);
        assertOptionalGraphText(node.artifact_digest, "artifact digest");
        assertStringList(node.features, "resolved node features");
        const nodeId = resolvedKey(node.id);
        if (nodeIds.has(nodeId)) {
          throw new Error("The resolved dependency graph contains a duplicate package identity.");
        }
        nodeIds.add(nodeId);
      }
      const rootIds = new Set();
      for (const root of document.roots) {
        assertIdentity(root, true);
        const rootId = resolvedKey(root);
        if (!nodeIds.has(rootId)) {
          throw new Error("The resolved dependency graph names a root outside its node set.");
        }
        if (rootIds.has(rootId)) {
          throw new Error("The resolved dependency graph contains a duplicate root identity.");
        }
        rootIds.add(rootId);
      }
      for (const edge of document.edges) {
        assertIdentity(edge?.from, true);
        assertIdentity(edge?.to, true);
        assertOptionalGraphText(edge.requirement, "resolved dependency requirement");
        assertOptionalGraphText(edge.target, "resolved dependency target");
        assertDependencyKind(edge.kind);
        assertOptionalBoolean(edge.optional, "resolved dependency optional marker");
        assertStringList(edge.features, "resolved dependency features");
        if (!nodeIds.has(resolvedKey(edge.from)) || !nodeIds.has(resolvedKey(edge.to))) {
          throw new Error("The resolved dependency graph contains an edge outside its node set.");
        }
      }
      return;
    }
    throw new Error("The server returned an unknown dependency graph view.");
  }

  assertDocumentCapacity(document) {
    let incomingNodes = 0;
    let incomingEdges = 0;
    let incomingRoots = 0;
    if (document.view === "declared") {
      incomingNodes = 1 + (Array.isArray(document.dependencies) ? document.dependencies.length : 0);
      incomingEdges = Array.isArray(document.dependencies) ? document.dependencies.length : 0;
    } else if (document.view === "resolved") {
      incomingNodes = Array.isArray(document.nodes) ? document.nodes.length : 0;
      incomingEdges = Array.isArray(document.edges) ? document.edges.length : 0;
      incomingRoots = Array.isArray(document.roots) ? document.roots.length : 0;
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
    if (incomingRoots > MAX_GRAPH_NODES) {
      throw new Error(`The loaded topology exceeds the ${MAX_GRAPH_NODES}-root browser limit.`);
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
      prerelease: false,
      yanked: false,
      private: false,
      license: "",
      updatedAt: "",
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
      prerelease: isPrereleaseVersion(node.id.version || ""),
      yanked: false,
      private: false,
      license: "",
      updatedAt: "",
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

  fetchDocument(url, options = {}) {
    const requestOptions = { ...options, generation: this.fetchGeneration };
    if (!options.serialized) return this.fetchDocumentNow(url, requestOptions);
    const run = () => this.fetchDocumentNow(url, requestOptions);
    const result = this.serialFetchTail.then(run, run);
    this.serialFetchTail = result.catch(() => undefined);
    return result;
  }

  async fetchDocumentNow(url, options = {}) {
    const generation = options.generation ?? this.fetchGeneration;
    if (generation !== this.fetchGeneration) {
      throw new Error("The dependency graph request was cancelled.");
    }
    const noStore = Boolean(options.noStore);
    if (noStore) this.cache.delete(url);
    const cached = noStore ? null : this.cache.get(url);
    const headers = { Accept: "application/vnd.zpkg.dependency-graph.v1+json" };
    if (cached?.etag) headers["If-None-Match"] = cached.etag;
    const controller = new AbortController();
    this.fetchControllers.add(controller);
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, FETCH_TIMEOUT_MS);
    let response;
    try {
      const request = {
        headers,
        credentials: "same-origin",
        signal: controller.signal,
      };
      if (noStore) request.cache = "no-store";
      response = await fetch(url, request);
    } catch (error) {
      if (error?.name === "AbortError") {
        throw new Error(
          timedOut
            ? "The dependency graph request timed out."
            : "The dependency graph request was cancelled."
        );
      }
      throw error;
    } finally {
      clearTimeout(timeout);
      this.fetchControllers.delete(controller);
    }
    if (generation !== this.fetchGeneration) {
      throw new Error("The dependency graph request was cancelled.");
    }
    const responseNoStore = cacheControlDisallowsStorage(
      response.headers.get("cache-control") || ""
    );
    if (response.status === 304 && responseNoStore) {
      this.cache.delete(url);
      if (!options.retriedWithoutCache) {
        return this.fetchDocumentNow(url, {
          ...options,
          generation,
          noStore: true,
          retriedWithoutCache: true,
        });
      }
      throw new Error("The dependency graph server revalidated a non-storable response.");
    }
    if (response.status === 304 && cached) {
      const responseEtag = response.headers.get("etag") || "";
      const responseDigest = response.headers.get("x-zpkg-graph-digest") || "";
      const responseAuthority = response.headers.get("x-zpkg-graph-authoritative") || "";
      const responseLength = parseContentLength(response.headers.get("content-length"));
      const responseVersion = response.headers.get("x-zpkg-selected-version") || "";
      if (!isStrongGraphEtag(responseEtag) || responseEtag !== cached.etag) {
        throw new Error("The dependency graph cache validator was missing or changed.");
      }
      if (!isGraphDigest(responseDigest) || responseDigest !== cached.digest) {
        throw new Error("The cached dependency graph semantic identity was missing or changed.");
      }
      if (responseAuthority !== "true" || responseAuthority !== cached.authoritative) {
        throw new Error("The cached dependency graph authority marker was missing or changed.");
      }
      if (responseLength === null || responseLength !== cached.contentLength) {
        throw new Error("The cached dependency graph representation length was missing or changed.");
      }
      if (responseVersion !== cached.selectedVersion) {
        throw new Error("The cached dependency graph selected version changed unexpectedly.");
      }
      this.rememberDocument(url, cached);
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
    const authoritative = response.headers.get("x-zpkg-graph-authoritative") || "";
    if (authoritative !== "true") {
      throw new Error("The server returned a non-authoritative dependency graph representation.");
    }
    const contentLength = parseContentLength(response.headers.get("content-length"));
    if (contentLength === null || contentLength > MAX_GRAPH_DOCUMENT_BYTES) {
      throw new Error("The dependency graph response did not carry a valid representation length.");
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength !== contentLength) {
      throw new Error("The dependency graph response length did not match its representation metadata.");
    }
    let document;
    try {
      document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    } catch {
      throw new Error("The dependency graph response was not valid UTF-8 JSON.");
    }
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
      authoritative,
      contentLength,
      selectedVersion: response.headers.get("x-zpkg-selected-version") || "",
    };
    if (noStore || responseNoStore) this.cache.delete(url);
    else this.rememberDocument(url, result);
    return result;
  }

  rememberDocument(url, result) {
    this.cache.delete(url);
    this.cache.set(url, result);
    while (this.cache.size > MAX_DOCUMENT_CACHE_ENTRIES) {
      this.cache.delete(this.cache.keys().next().value);
    }
  }

  applyParsedViewState(state) {
    this.layoutName = state.layout;
    this.searchValue = state.search;
    this.searchTerm = state.search.trim().toLowerCase();
    this.enabledKinds = new Set(state.kinds);
    this.includeOptional = state.includeOptional;
    this.channel = state.channel;
    this.activeQuery = state.query;
    this.queryAnchorId = state.queryAnchor;
    this.pendingQueryAnchorId = state.queryAnchor;
    this.pendingSelectedId = state.selected;
    this.pendingPathStartId = state.pathStart;
    if (this.mode === "package") {
      this.dataset.version = this.versions.some((item) => item.version === state.version)
        ? state.version
        : this.defaultVersion;
    }
  }

  currentViewState() {
    return {
      layout: this.layoutName,
      search: this.$?.('[data-control="search"]')?.value.trim() || this.searchValue,
      kinds: [...this.enabledKinds],
      includeOptional: this.includeOptional,
      query: this.activeQuery,
      queryAnchor: this.queryAnchorId,
      selected: this.selectedId || "",
      pathStart: this.pathStartId || "",
      channel: this.channel,
      version: this.mode === "package" ? this.dataset.version || "" : "",
    };
  }

  updateControlsFromState() {
    const search = this.$?.('[data-control="search"]');
    if (search) search.value = this.searchValue;
    const version = this.$?.('[data-control="version"]');
    if (version && [...version.options].some((option) => option.value === this.dataset.version)) {
      version.value = this.dataset.version;
    }
    const channel = this.$?.('[data-control="channel"]');
    if (channel) channel.value = this.channel;
    this.$$?.('[data-layout]')?.forEach((button) =>
      button.setAttribute("aria-pressed", String(button.dataset.layout === this.layoutName))
    );
    this.$$?.('[data-kind]')?.forEach(
      (checkbox) => (checkbox.checked = this.enabledKinds.has(checkbox.dataset.kind))
    );
    const optional = this.$?.('[data-control="optional"]');
    if (optional) optional.checked = this.includeOptional;
  }

  syncViewUrl() {
    if (!globalThis.location || !globalThis.history?.replaceState) return;
    const url = graphViewUrl(location.href, this.currentViewState());
    history.replaceState(history.state, "", url);
  }

  savedViewKey() {
    return `zpkg.graph.saved:${globalThis.location?.pathname || this.id}`;
  }

  saveView() {
    safeStorageSet(this.savedViewKey(), JSON.stringify(this.currentViewState()));
    this.syncViewUrl();
    this.requestStateFragment("save");
    this.setStatus("Saved this dependency graph view in this browser.", "ready");
  }

  restoreSavedView() {
    const saved = safeStorageGet(this.savedViewKey());
    if (!saved) {
      this.setStatus("No saved dependency graph view exists for this page.", "error");
      return;
    }
    try {
      const state = JSON.parse(saved);
      if (!globalThis.location || !globalThis.history?.replaceState) return;
      history.replaceState(history.state, "", graphViewUrl(location.href, state));
      this.restoreViewFromLocation();
      this.requestStateFragment("restore");
      this.setStatus("Restored the saved dependency graph view.", "ready");
    } catch {
      this.setStatus("The saved dependency graph view is invalid.", "error");
    }
  }

  async copyShareLink() {
    this.syncViewUrl();
    this.requestStateFragment("share");
    try {
      if (!globalThis.navigator?.clipboard?.writeText) {
        throw new Error("Clipboard access is unavailable.");
      }
      await navigator.clipboard.writeText(location.href);
      this.setStatus("Copied a reproducible dependency graph link.", "ready");
    } catch {
      this.setStatus("Copy is unavailable; copy the current address from the browser.", "error");
    }
  }

  restoreViewFromLocation() {
    const previousVersion = this.dataset.version || "";
    const previousChannel = this.channel;
    const state = parseGraphViewState(globalThis.location?.href, {
      layout: this.layoutName,
    });
    this.applyParsedViewState(state);
    this.updateControlsFromState();
    if (this.mode === "package" && this.dataset.version !== previousVersion) {
      this.loadPackage(this.dataset.version);
      return;
    }
    if (this.mode === "scope" && this.channel !== previousChannel) {
      this.loadScope();
      return;
    }
    this.applyLayout(false);
    this.restoreLoadedViewState();
  }

  afterGraphLoaded(message) {
    this.dataset.graphLoadMs = (performanceNow() - this.loadStartedAt).toFixed(1);
    this.applyLayout(false);
    this.updateMetrics();
    this.renderAccessibleTable();
    this.restoreLoadedViewState(message);
  }

  restoreLoadedViewState(message = "Dependency graph view restored.") {
    const selected = this.pendingSelectedId && this.nodes.has(this.pendingSelectedId)
      ? this.pendingSelectedId
      : "";
    if (selected) this.selectNode(selected, false);
    else if (!this.selectedId && !this.restoreSelectionFromHash() && this.roots.size) {
      this.selectNode([...this.roots][0], false);
    }
    this.pathStartId = this.nodes.has(this.pendingPathStartId) ? this.pendingPathStartId : null;
    this.queryAnchorId = this.nodes.has(this.pendingQueryAnchorId)
      ? this.pendingQueryAnchorId
      : this.queryAnchorId;
    this.pendingSelectedId = "";
    this.pendingPathStartId = "";
    this.pendingQueryAnchorId = "";
    if (this.activeQuery) {
      this.runQuery(this.activeQuery, { sync: false, fit: false, restoring: true });
    } else {
      this.renderQuerySummary();
      this.setStatus(message, "ready");
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
    const renderStartedAt = performanceNow();
    this.edgeLayer.replaceChildren();
    this.nodeLayer.replaceChildren();
    this.renderedNodeElements.clear();
    this.renderedEdgesByNode.clear();
    const allEdges = this.filteredEdges(true);
    const allVisibleNodes = this.visibleNodeSet(allEdges);
    const matches = this.searchMatches();
    const visibleNodes = boundedRenderedNodeSet(
      allVisibleNodes,
      this.nodes,
      allEdges,
      this.roots,
      this.selectedId,
      matches,
      MAX_RENDERED_NODES
    );
    const edges = allEdges
      .filter((edge) => visibleNodes.has(edge.from) && visibleNodes.has(edge.to))
      .slice(0, MAX_RENDERED_EDGES);
    this.updateDegradationSummary(
      allVisibleNodes.size,
      allEdges.length,
      visibleNodes.size,
      edges.length
    );
    this.$('[data-role="empty"]').hidden = allVisibleNodes.size > 0;
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
        "marker-end": `url(#${this.identifiers.arrow})`,
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
        "aria-pressed": String(selected),
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
    this.renderQuerySummary();
    this.dataset.graphRenderMs = (performanceNow() - renderStartedAt).toFixed(1);
  }

  updateDegradationSummary(nodeCount, edgeCount, renderedNodeCount, renderedEdgeCount) {
    if (!this.degradation) return;
    const bounded = renderedNodeCount < nodeCount || renderedEdgeCount < edgeCount;
    this.degradation.hidden = !bounded;
    this.degradation.textContent = bounded
      ? `Large-graph overview: the canvas shows ${renderedNodeCount} of ${nodeCount} packages and ${renderedEdgeCount} of ${edgeCount} relationships, prioritizing scope roots, matches, selection, and central packages. Queries and downloadable semantic data still use the full loaded graph.`
      : "";
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
      if (updateHash) this.syncViewUrl();
      return;
    }
    if (!this.nodes.has(id)) return;
    this.selectedId = id;
    this.renderGraph();
    if (updateHash) this.syncViewUrl();
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
      globalThis.htmx?.trigger?.(this.inspector, "htmx:abort");
      this.fragmentSequences.set("inspector", (this.fragmentSequences.get("inspector") || 0) + 1);
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
        <dt>License</dt><dd>${escapeHtml(node.license || "—")}</dd>
        <dt>Last metadata update</dt><dd>${escapeHtml(formatMetadataDate(node.updatedAt) || "—")}</dd>
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
    this.bindInspectorControls(node);
    this.requestInspectorFragment(node, outgoing, incoming, requirements, features, expandable);
  }

  bindInspectorControls(node) {
    this.inspector.querySelector('[data-inspector-action="expand"]')?.addEventListener("click", () =>
      this.expandLatest(node)
    );
    this.inspector.querySelectorAll('[data-select-node]').forEach((button) =>
      button.addEventListener("click", () => this.selectNode(button.dataset.selectNode, true))
    );
  }

  requestInspectorFragment(node, outgoing, incoming, requirements, features, expandable) {
    const neighbors = outgoing
      .slice(0, 18)
      .map((edge) => ({ edge, target: this.nodes.get(edge.to) }))
      .filter(({ target }) => target)
      .map(({ edge, target }) => ({
        id: edge.to,
        org: target.org,
        name: target.name,
        kind: edge.kind,
        requirement: edge.requirement || "",
      }));
    const nodeId = node.id;
    const payload = {
      org: node.org,
      name: node.name,
      registry_id: node.registryId,
      version: node.version || "",
      requirements,
      dependencies: outgoing.length,
      dependents: incoming.length,
      features,
      license: node.license || "",
      updated_at: formatMetadataDate(node.updatedAt),
      artifact: shortDigest(node.artifactDigest) || "",
      synthetic: Boolean(node.synthetic),
      expandable,
      outgoing: neighbors,
    };
    this.requestFragment("inspector", this.inspector, { node: JSON.stringify(payload) }, () => {
      const current = this.nodes.get(this.selectedId);
      if (!current || current.id !== nodeId) {
        this.renderInspector();
        return;
      }
      this.bindInspectorControls(current);
    });
  }

  async expandLatest(node) {
    const sequence = this.loadSequence;
    node.expanded = true;
    this.setStatus(`Expanding latest declared graph for ${node.org}/${node.name}…`, "loading");
    try {
      // A latest neighbor may be private, but visibility is intentionally not
      // exposed in the graph document. Serialize these reads so rotating the
      // opaque browser refresh handle cannot race across rapid expansions.
      const result = await this.fetchDocument(latestDocumentUrl(node.org, node.name), {
        serialized: true,
        noStore: true,
      });
      if (sequence !== this.loadSequence) return;
      if (!result.selectedVersion) {
        throw new Error("The latest dependency graph response omitted its selected version.");
      }
      assertDeclaredDocumentCoordinate(
        result.document,
        node.org,
        node.name,
        result.selectedVersion
      );
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
      if (sequence !== this.loadSequence) return;
      node.expanded = false;
      this.fail(error);
    }
  }

  runQuery(query, options = {}) {
    if (query === "clear") {
      this.focusNodes = null;
      this.focusEdges = null;
      this.focusLabel = "";
      this.activeQuery = "";
      this.queryAnchorId = "";
      this.queryPage = 0;
      this.setStatus("Query focus cleared.", "ready");
      this.renderGraph();
      if (options.sync !== false) this.syncViewUrl();
      return;
    }
    const selectionIndependent = new Set([
      "cycles",
      "internal",
      "external",
      "duplicates",
      "prerelease",
      "yanked",
      "centrality",
      "licenses",
      "license-review",
      "unmaintained",
    ]);
    const selected = options.restoring && this.nodes.has(this.queryAnchorId)
      ? this.queryAnchorId
      : this.selectedId || [...this.roots][0];
    if (!selected && !selectionIndependent.has(query)) {
      this.setStatus("Select a package before running this query.", "error");
      return;
    }

    if (query === "pin-path") {
      this.pathStartId = selected;
      this.setStatus(`Path start pinned at ${nodeLabel(this.nodes.get(selected))}. Select an endpoint and run Shortest path.`, "ready");
      this.syncViewUrl();
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
    } else if (["internal", "external", "duplicates", "prerelease", "yanked", "centrality", "licenses", "license-review", "unmaintained"].includes(query)) {
      result = aggregateQueryNodes(query, this.nodes, this.roots, queryEdges);
      label = {
        internal: "Packages published in this scope",
        external: "Dependencies outside this scope",
        duplicates: "Packages with multiple loaded versions",
        prerelease: "Packages exposing pre-release versions",
        yanked: "Packages exposing yanked versions",
        centrality: "Highest-centrality packages in the loaded graph",
        licenses: "License distribution for packages published in this scope",
        "license-review": "Scope packages with missing or mixed license metadata requiring review",
        unmaintained: `Scope packages without a metadata update in ${UNMAINTAINED_AFTER_DAYS} days`,
      }[query];
    }
    if (!result) return;
    this.focusNodes = result;
    this.focusLabel = label;
    this.activeQuery = query;
    this.queryAnchorId = selectionIndependent.has(query) ? "" : selected;
    this.queryPage = 0;
    this.setStatus(`${label}: ${result.size} package(s).`, result.size ? "ready" : "error");
    this.renderGraph();
    if (options.fit !== false) requestAnimationFrame(() => this.fitGraph());
    if (options.sync !== false) this.syncViewUrl();
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
    this.activeQuery = "";
    this.queryAnchorId = "";
    this.queryPage = 0;
    this.pathStartId = null;
    this.searchTerm = "";
    this.searchValue = "";
    this.$('[data-control="search"]').value = "";
    this.enabledKinds = new Set(Object.keys(KIND_LABELS));
    this.$$('[data-kind]').forEach((checkbox) => (checkbox.checked = true));
    this.includeOptional = true;
    this.$('[data-control="optional"]').checked = true;
    this.renderGraph();
    this.fitGraph();
    this.syncViewUrl();
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
    const target = this.$('[data-role="table"]');
    const shownEdges = this.edges.slice(0, MAX_ACCESSIBLE_EDGES);
    const rows = shownEdges
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
    const boundedNote = shownEdges.length < this.edges.length
      ? `<p role="status">Showing the first ${shownEdges.length} of ${this.edges.length} relationships to keep this page responsive. Use the canonical CSV exports for complete semantic edge data.</p>`
      : "";
    target.innerHTML = this.edges.length
      ? `${boundedNote}<table><caption>Loaded dependency relationships</caption><thead><tr><th scope="col">From</th><th scope="col">To</th><th scope="col">Kind</th><th scope="col">Requirement</th><th scope="col">Optional</th></tr></thead><tbody>${rows}</tbody></table>`
      : `<p>No dependency relationships are loaded.</p>`;
    const fragmentRows = this.edges
      .slice(0, MAX_FRAGMENT_TABLE_EDGES)
      .map((edge) => ({ edge, from: this.nodes.get(edge.from), to: this.nodes.get(edge.to) }))
      .filter(({ from, to }) => from && to)
      .map(({ edge, from, to }) => ({
        from_org: from.org,
        from_name: from.name,
        to_org: to.org,
        to_name: to.name,
        kind: edge.kind,
        requirement: edge.requirement || "",
        optional: Boolean(edge.optional),
      }));
    this.requestFragment("table", target, {
      total: this.edges.length,
      rows: JSON.stringify(fragmentRows),
    });
  }

  renderQuerySummary() {
    if (!this.querySummary) return;
    if (!this.activeQuery || !this.focusNodes) {
      globalThis.htmx?.trigger?.(this.querySummary, "htmx:abort");
      this.fragmentSequences.set("query", (this.fragmentSequences.get("query") || 0) + 1);
      this.querySummary.hidden = true;
      this.querySummary.replaceChildren();
      return;
    }
    const nodes = [...this.focusNodes]
      .map((id) => this.nodes.get(id))
      .filter(Boolean)
      .sort((a, b) => nodeLabel(a).localeCompare(nodeLabel(b)) || a.version.localeCompare(b.version));
    const pageCount = Math.max(1, Math.ceil(nodes.length / QUERY_RESULT_PAGE_SIZE));
    this.queryPage = clamp(this.queryPage, 0, pageCount - 1);
    const start = this.queryPage * QUERY_RESULT_PAGE_SIZE;
    const pageNodes = nodes.slice(start, start + QUERY_RESULT_PAGE_SIZE);
    const outgoingCounts = degreeCounts(this.edges, "from");
    const incomingCounts = degreeCounts(this.edges, "to");
    const rows = pageNodes
      .map(
        (node) => `<tr>
          <th scope="row"><a href="${packagePageUrl(node.org, node.name)}">${escapeHtml(nodeLabel(node))}</a></th>
          <td>${escapeHtml(node.version || "unresolved")}</td>
          <td>${escapeHtml(node.license || "—")}</td>
          <td>${escapeHtml(formatMetadataDate(node.updatedAt) || "—")}</td>
          <td>${outgoingCounts.get(node.id) || 0}</td>
          <td>${incomingCounts.get(node.id) || 0}</td>
          <td><button type="button" data-query-select="${escapeHtml(node.id)}">Inspect</button></td>
        </tr>`
      )
      .join("");
    const first = nodes.length ? start + 1 : 0;
    const last = Math.min(nodes.length, start + QUERY_RESULT_PAGE_SIZE);
    this.querySummary.innerHTML = `
      <div class="dg-query-summary-head">
        <div>
          <p class="dg-eyebrow">Accessible query result</p>
          <h3 id="${this.identifiers.namespace}-query-title">${escapeHtml(this.focusLabel)}</h3>
          <p>${nodes.length ? `Showing ${first}–${last} of ${nodes.length} packages.` : "No packages matched this analysis."}</p>
        </div>
        ${
          pageCount > 1
            ? `<nav aria-label="Query result pages">
                 <button type="button" data-query-page="previous"${this.queryPage === 0 ? " disabled" : ""}>Previous</button>
                 <span>Page ${this.queryPage + 1} of ${pageCount}</span>
                 <button type="button" data-query-page="next"${this.queryPage + 1 === pageCount ? " disabled" : ""}>Next</button>
               </nav>`
            : ""
        }
      </div>
      ${
        rows
          ? `<div class="dg-query-table"><table><caption>${escapeHtml(this.focusLabel)}</caption><thead><tr><th scope="col">Package</th><th scope="col">Version</th><th scope="col">License</th><th scope="col">Updated</th><th scope="col">Dependencies</th><th scope="col">Dependents</th><th scope="col">Action</th></tr></thead><tbody>${rows}</tbody></table></div>`
          : ""
      }`;
    this.querySummary.hidden = false;
    this.bindQuerySummaryControls();
    const fragmentRows = pageNodes.map((node) => ({
      id: node.id,
      org: node.org,
      name: node.name,
      version: node.version || "",
      license: node.license || "",
      updated_at: formatMetadataDate(node.updatedAt),
      dependencies: outgoingCounts.get(node.id) || 0,
      dependents: incomingCounts.get(node.id) || 0,
    }));
    this.requestFragment(
      "query",
      this.querySummary,
      {
        label: this.focusLabel,
        title_id: `${this.identifiers.namespace}-query-title`,
        page: this.queryPage,
        total: nodes.length,
        rows: JSON.stringify(fragmentRows),
      },
      () => this.bindQuerySummaryControls()
    );
  }

  bindQuerySummaryControls() {
    this.querySummary.querySelectorAll('[data-query-select]').forEach((button) =>
      button.addEventListener("click", () => this.selectNode(button.dataset.querySelect, true))
    );
    this.querySummary.querySelectorAll('[data-query-page]').forEach((button) =>
      button.addEventListener("click", () => {
        this.queryPage += button.dataset.queryPage === "next" ? 1 : -1;
        this.renderQuerySummary();
      })
    );
  }

  updateExportLinks() {
    if (this.mode !== "package") return;
    const version = this.dataset.version || this.versions[0]?.version || "";
    this.$$('[data-export]').forEach((link) => {
      link.href = packageExportUrl(this.dataset.org, this.dataset.package, version, link.dataset.export);
      link.download = `${safeFilename(this.dataset.org)}_${safeFilename(this.dataset.package)}_${safeFilename(version)}.dependency-graph.${extensionFor(link.dataset.export)}`;
    });
  }

  projectionDocument() {
    const allEdges = this.filteredEdges(true);
    const allVisibleNodes = this.visibleNodeSet(allEdges);
    const visibleNodes = boundedRenderedNodeSet(
      allVisibleNodes,
      this.nodes,
      allEdges,
      this.roots,
      this.selectedId,
      this.searchMatches(),
      MAX_RENDERED_NODES
    );
    const edges = allEdges
      .filter((edge) => visibleNodes.has(edge.from) && visibleNodes.has(edge.to))
      .slice(0, MAX_RENDERED_EDGES);
    return projectionSvgDocument({
      nodes: [...visibleNodes].map((id) => this.nodes.get(id)).filter(Boolean),
      edges: edges.filter(
        (edge) => visibleNodes.has(edge.from) && visibleNodes.has(edge.to)
      ),
      positions: this.positions,
      roots: this.roots,
      selectedId: this.selectedId,
      title: this.dataset.scopeTitle || "Dependency graph visible projection",
    });
  }

  async exportProjection(format) {
    const projection = this.projectionDocument();
    if (!projection) {
      throw new Error("Load at least one package before exporting the visible projection.");
    }
    const nameParts =
      this.mode === "package"
        ? [this.dataset.org, this.dataset.package, this.dataset.version]
        : [this.dataset.scopeKind, this.dataset.scopeTitle];
    const filename = projectionFilename(nameParts, format);
    if (format === "svg") {
      downloadBlob(
        new Blob([projection.svg], { type: "image/svg+xml;charset=utf-8" }),
        filename
      );
      this.setStatus("Downloaded the visible graph projection as SVG.", "ready");
      return;
    }
    if (format !== "png") {
      throw new Error("That visible projection format is not supported.");
    }

    const image = new Image();
    const source = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(projection.svg)}`;
    await new Promise((resolve, reject) => {
      image.onload = resolve;
      image.onerror = () =>
        reject(new Error("The browser could not rasterize this graph projection."));
      image.src = source;
    });
    const canvas = document.createElement("canvas");
    canvas.width = projection.width;
    canvas.height = projection.height;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("The browser does not provide a PNG export canvas.");
    context.drawImage(image, 0, 0, projection.width, projection.height);
    const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
    if (!blob) throw new Error("The browser could not encode this graph projection as PNG.");
    downloadBlob(blob, filename);
    this.setStatus("Downloaded the visible graph projection as PNG.", "ready");
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
    if (type === "pan" && !moved) this.selectNode(null, true);
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
  return JSON.stringify([
    identity.registry_id || "registry:unknown",
    identity.org,
    identity.name,
  ]);
}

function resolvedKey(identity) {
  return JSON.stringify([
    identity.registry_id || "registry:unknown",
    identity.org,
    identity.name,
    identity.version,
  ]);
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
  return JSON.stringify([
    edge.from,
    edge.to,
    edge.kind,
    edge.requirement,
    edge.target,
    edge.optional,
  ]);
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

function projectionSvgDocument({
  nodes,
  edges,
  positions,
  roots = new Set(),
  selectedId = null,
  title = "Dependency graph visible projection",
}) {
  const positionedNodes = nodes
    .filter((node) => {
      const position = positions.get(node.id);
      return position && Number.isFinite(position.x) && Number.isFinite(position.y);
    })
    .sort((left, right) => left.id.localeCompare(right.id));
  if (!positionedNodes.length) return null;

  const included = new Set(positionedNodes.map((node) => node.id));
  const nodesById = new Map(positionedNodes.map((node) => [node.id, node]));
  const positionedEdges = edges
    .filter(
      (edge) =>
        included.has(edge.from) &&
        included.has(edge.to) &&
        positions.has(edge.from) &&
        positions.has(edge.to)
    )
    .sort((left, right) => edgeIdentity(left).localeCompare(edgeIdentity(right)));
  const bounds = graphBounds(positionedNodes.map((node) => positions.get(node.id)));
  const padding = 72;
  const minX = bounds.minX - NODE_WIDTH / 2 - padding;
  const minY = bounds.minY - NODE_HEIGHT / 2 - padding;
  const viewWidth = Math.max(
    1,
    bounds.maxX - bounds.minX + NODE_WIDTH + padding * 2
  );
  const viewHeight = Math.max(
    1,
    bounds.maxY - bounds.minY + NODE_HEIGHT + padding * 2
  );
  const scale = Math.min(
    2,
    MAX_PROJECTION_DIMENSION / viewWidth,
    MAX_PROJECTION_DIMENSION / viewHeight
  );
  const width = Math.max(1, Math.round(viewWidth * scale));
  const height = Math.max(1, Math.round(viewHeight * scale));
  const kindColors = {
    runtime: "#7dd3fc",
    build: "#fbbf24",
    development: "#c4b5fd",
    peer: "#5eead4",
    tooling: "#fb7185",
  };

  const edgeMarkup = positionedEdges
    .map((edge) => {
      const stroke = kindColors[edge.kind] || kindColors.runtime;
      const dash = edge.synthetic ? "2 6" : edge.optional ? "7 6" : "none";
      return `<path d="${escapeHtml(
        edgePath(positions.get(edge.from), positions.get(edge.to))
      )}" fill="none" stroke="${stroke}" stroke-width="1.8" stroke-opacity="0.72" stroke-dasharray="${dash}" marker-end="url(#projection-arrow)"><title>${escapeHtml(
        `${nodeLabel(nodesById.get(edge.from))} → ${nodeLabel(
          nodesById.get(edge.to)
        )} · ${edge.kind}`
      )}</title></path>`;
    })
    .join("");
  const nodeMarkup = positionedNodes
    .map((node) => {
      const position = positions.get(node.id);
      const selected = node.id === selectedId;
      const root = roots.has(node.id);
      const stroke = selected ? "#ff7a1a" : root ? "#b8662d" : "#2e3b4c";
      const dot = node.synthetic ? "#fef08a" : selected ? "#ff7a1a" : "#8fd3f4";
      const dash = node.synthetic ? ' stroke-dasharray="4 4"' : "";
      return `<g transform="translate(${position.x - NODE_WIDTH / 2} ${
        position.y - NODE_HEIGHT / 2
      })"><rect width="${NODE_WIDTH}" height="${NODE_HEIGHT}" rx="14" fill="${
        selected ? "#15130f" : "#0c1017"
      }" stroke="${stroke}" stroke-width="${selected ? 2.2 : 1.2}"${dash}/><circle cx="19" cy="21" r="5" fill="${dot}"/><text x="32" y="25" fill="#f2f2f0" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="12" font-weight="600">${escapeHtml(
        truncate(nodeLabel(node), 27)
      )}</text><text x="18" y="47" fill="#8fa1b5" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="10">${escapeHtml(
        truncate(nodeMeta(node), 34)
      )}</text>${
        root
          ? `<text x="${
              NODE_WIDTH - 12
            }" y="17" text-anchor="end" fill="#ff7a1a" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="8" font-weight="700">ROOT</text>`
          : ""
      }><title>${escapeHtml(`${nodeLabel(node)}\n${nodeMeta(node)}`)}</title></g>`;
    })
    .join("");
  const safeTitle = escapeHtml(title);
  const svgDocument = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="${minX} ${minY} ${viewWidth} ${viewHeight}" role="img" aria-labelledby="projection-title projection-description"><title id="projection-title">${safeTitle}</title><desc id="projection-description">Visible dependency graph projection with ${positionedNodes.length} packages and ${positionedEdges.length} relationships.</desc><defs><pattern id="projection-grid" width="32" height="32" patternUnits="userSpaceOnUse"><path d="M 32 0 L 0 0 0 32" fill="none" stroke="#8fd3f4" stroke-opacity="0.07" stroke-width="1"/></pattern><marker id="projection-arrow" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto" markerUnits="strokeWidth"><path d="M0,0 L9,4.5 L0,9 z" fill="#8fa4b9"/></marker></defs><rect x="${minX}" y="${minY}" width="${viewWidth}" height="${viewHeight}" rx="18" fill="#07080c"/><rect x="${minX}" y="${minY}" width="${viewWidth}" height="${viewHeight}" rx="18" fill="url(#projection-grid)"/>${edgeMarkup}${nodeMarkup}</svg>`;
  return { svg: svgDocument, width, height, viewWidth, viewHeight };
}

function projectionFilename(parts, format) {
  const extension = format === "png" ? "png" : "svg";
  const base = parts.filter(Boolean).map(safeFilename).join("_") || "dependency_graph";
  return `${base}.dependency-graph.visible.${extension}`;
}

function downloadBlob(blob, filename) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.hidden = true;
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1_000);
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

function sourceMatchesChannel(source, channel) {
  if (channel === "all") return true;
  const prerelease =
    typeof source.prerelease === "boolean"
      ? source.prerelease
      : isPrereleaseVersion(source.version || "");
  return channel === "prerelease" ? prerelease : !prerelease;
}

function isPrereleaseVersion(version) {
  const value = String(version || "");
  if (!/^\d+(?:\.\d+)*/.test(value)) return false;
  const withoutBuild = value.split("+", 1)[0];
  return withoutBuild.includes("-");
}

function formatMetadataDate(value) {
  const timestamp = Date.parse(String(value || ""));
  return Number.isFinite(timestamp) ? new Date(timestamp).toISOString().slice(0, 10) : "";
}

function isUnmaintainedNode(node, now = Date.now()) {
  const updatedAt = Date.parse(String(node?.updatedAt || ""));
  return (
    Number.isFinite(updatedAt) &&
    Number.isFinite(now) &&
    now - updatedAt > UNMAINTAINED_AFTER_DAYS * MILLISECONDS_PER_DAY
  );
}

function degreeCounts(edges, key) {
  const counts = new Map();
  for (const edge of edges) counts.set(edge[key], (counts.get(edge[key]) || 0) + 1);
  return counts;
}

function aggregateQueryNodes(query, nodes, roots, edges) {
  const result = new Set();
  if (query === "internal") {
    for (const id of roots) if (nodes.has(id)) result.add(id);
    return result;
  }
  if (query === "external") {
    for (const id of nodes.keys()) if (!roots.has(id)) result.add(id);
    return result;
  }
  if (query === "prerelease") {
    for (const [id, node] of nodes) {
      if (node.prerelease || isPrereleaseVersion(node.version)) result.add(id);
    }
    return result;
  }
  if (query === "yanked") {
    for (const [id, node] of nodes) if (node.yanked) result.add(id);
    return result;
  }
  if (query === "duplicates") {
    const coordinates = new Map();
    for (const [id, node] of nodes) {
      const coordinate = JSON.stringify([node.registryId, node.org, node.name]);
      if (!coordinates.has(coordinate)) coordinates.set(coordinate, []);
      coordinates.get(coordinate).push([id, node.version]);
    }
    for (const instances of coordinates.values()) {
      const versions = new Set(instances.map(([, version]) => version).filter(Boolean));
      if (versions.size > 1) instances.forEach(([id]) => result.add(id));
    }
    return result;
  }
  if (query === "licenses") {
    for (const id of roots) if (nodes.has(id)) result.add(id);
    return result;
  }
  if (query === "license-review") {
    const scopeNodes = [...roots].map((id) => [id, nodes.get(id)]).filter(([, node]) => node);
    const declaredLicenses = new Set(
      scopeNodes.map(([, node]) => String(node.license || "").trim().toLowerCase()).filter(Boolean)
    );
    for (const [id, node] of scopeNodes) {
      if (!String(node.license || "").trim() || declaredLicenses.size > 1) result.add(id);
    }
    return result;
  }
  if (query === "unmaintained") {
    for (const id of roots) {
      const node = nodes.get(id);
      if (node && isUnmaintainedNode(node)) result.add(id);
    }
    return result;
  }
  if (query === "centrality") {
    const scores = new Map([...nodes.keys()].map((id) => [id, 0]));
    for (const edge of edges) {
      scores.set(edge.from, (scores.get(edge.from) || 0) + 1);
      scores.set(edge.to, (scores.get(edge.to) || 0) + 1);
    }
    const ranked = [...scores]
      .filter(([, score]) => score > 0)
      .sort((a, b) => b[1] - a[1] || nodeLabel(nodes.get(a[0])).localeCompare(nodeLabel(nodes.get(b[0]))));
    const count = Math.min(20, Math.max(1, Math.ceil(nodes.size * 0.1)));
    ranked.slice(0, count).forEach(([id]) => result.add(id));
    return result;
  }
  return result;
}

function boundedRenderedNodeSet(visible, nodes, edges, roots, selectedId, matches, limit) {
  if (visible.size <= limit) return new Set(visible);
  const scores = new Map([...visible].map((id) => [id, 0]));
  for (const edge of edges) {
    if (scores.has(edge.from)) scores.set(edge.from, scores.get(edge.from) + 1);
    if (scores.has(edge.to)) scores.set(edge.to, scores.get(edge.to) + 1);
  }
  const compare = (left, right) =>
    (scores.get(right) || 0) - (scores.get(left) || 0) ||
    nodeLabel(nodes.get(left)).localeCompare(nodeLabel(nodes.get(right))) ||
    left.localeCompare(right);
  const selected = selectedId && visible.has(selectedId) ? [selectedId] : [];
  const rootIds = [...roots].filter((id) => visible.has(id) && id !== selectedId).sort(compare);
  const matchIds = [...matches]
    .filter((id) => visible.has(id) && id !== selectedId && !roots.has(id))
    .sort(compare);
  const prioritized = new Set([...selected, ...rootIds, ...matchIds]);
  const remaining = [...visible].filter((id) => !prioritized.has(id)).sort(compare);
  return new Set([...prioritized, ...remaining].slice(0, limit));
}

function performanceNow() {
  return globalThis.performance?.now?.() ?? Date.now();
}

function boundedViewStateText(value) {
  return typeof value === "string" ? value.slice(0, MAX_VIEW_STATE_TEXT_LENGTH) : "";
}

function normalizeGraphViewState(state = {}, defaults = {}) {
  const fallbackLayout = GRAPH_LAYOUTS.has(defaults.layout) ? defaults.layout : "layered";
  const layout = GRAPH_LAYOUTS.has(state.layout) ? state.layout : fallbackLayout;
  const kinds = Array.isArray(state.kinds)
    ? [...new Set(state.kinds.filter((kind) => Object.hasOwn(KIND_LABELS, kind)))]
    : Object.keys(KIND_LABELS);
  return {
    layout,
    search: boundedViewStateText(state.search),
    kinds,
    includeOptional: state.includeOptional !== false,
    query: GRAPH_QUERIES.has(state.query) ? state.query : "",
    queryAnchor: boundedViewStateText(state.queryAnchor),
    selected: boundedViewStateText(state.selected),
    pathStart: boundedViewStateText(state.pathStart),
    channel: GRAPH_CHANNELS.has(state.channel) ? state.channel : "all",
    version: boundedViewStateText(state.version),
  };
}

function parseGraphViewState(value, defaults = {}) {
  let url;
  try {
    url = new URL(value || "/", "https://zpkg.invalid");
  } catch {
    url = new URL("https://zpkg.invalid/");
  }
  const parameters = url.searchParams;
  const kinds = parameters.has(GRAPH_STATE_PARAMETERS.kinds)
    ? (parameters.get(GRAPH_STATE_PARAMETERS.kinds) || "").split(",")
    : undefined;
  return normalizeGraphViewState(
    {
      layout: parameters.get(GRAPH_STATE_PARAMETERS.layout) || defaults.layout,
      search: parameters.get(GRAPH_STATE_PARAMETERS.search) || "",
      kinds,
      includeOptional: parameters.get(GRAPH_STATE_PARAMETERS.optional) !== "0",
      query: parameters.get(GRAPH_STATE_PARAMETERS.query) || "",
      queryAnchor: parameters.get(GRAPH_STATE_PARAMETERS.queryAnchor) || "",
      selected: parameters.get(GRAPH_STATE_PARAMETERS.selected) || "",
      pathStart: parameters.get(GRAPH_STATE_PARAMETERS.pathStart) || "",
      channel: parameters.get(GRAPH_STATE_PARAMETERS.channel) || "all",
      version: parameters.get(GRAPH_STATE_PARAMETERS.version) || "",
    },
    defaults
  );
}

function graphViewUrl(value, state) {
  const url = new URL(value || "/", "https://zpkg.invalid");
  const normalized = normalizeGraphViewState(state);
  const parameters = url.searchParams;
  parameters.set(GRAPH_STATE_PARAMETERS.layout, normalized.layout);
  parameters.set(GRAPH_STATE_PARAMETERS.kinds, [...normalized.kinds].sort().join(","));
  parameters.set(GRAPH_STATE_PARAMETERS.optional, normalized.includeOptional ? "1" : "0");
  for (const [key, field] of [
    [GRAPH_STATE_PARAMETERS.search, "search"],
    [GRAPH_STATE_PARAMETERS.query, "query"],
    [GRAPH_STATE_PARAMETERS.queryAnchor, "queryAnchor"],
    [GRAPH_STATE_PARAMETERS.selected, "selected"],
    [GRAPH_STATE_PARAMETERS.pathStart, "pathStart"],
    [GRAPH_STATE_PARAMETERS.version, "version"],
  ]) {
    if (normalized[field]) parameters.set(key, normalized[field]);
    else parameters.delete(key);
  }
  if (normalized.channel !== "all") {
    parameters.set(GRAPH_STATE_PARAMETERS.channel, normalized.channel);
  } else {
    parameters.delete(GRAPH_STATE_PARAMETERS.channel);
  }
  if (!url.hash || url.hash.startsWith("#dependency-graph=")) url.hash = "dependency-graph";
  return url.toString();
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

function parseContentLength(value) {
  if (typeof value !== "string" || !/^\d+$/.test(value)) return null;
  const length = Number(value);
  return Number.isSafeInteger(length) ? length : null;
}

function cacheControlDisallowsStorage(value) {
  return String(value)
    .split(",")
    .some(
      (directive) =>
        directive.trim().split("=", 1)[0].trim().toLowerCase() === "no-store"
    );
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

function assertIdentity(identity, requireVersion) {
  if (!identity || typeof identity !== "object") {
    throw new Error("The dependency graph contains an invalid package identity.");
  }
  assertGraphText(identity.registry_id, "package registry identity");
  assertGraphText(identity.org, "package organization");
  assertGraphText(identity.name, "package name");
  if (requireVersion) assertGraphText(identity.version, "package version");
}

function assertGraphText(value, field) {
  if (
    typeof value !== "string" ||
    !value.length ||
    GRAPH_TEXT_ENCODER.encode(value).byteLength > MAX_GRAPH_TEXT_BYTES ||
    !isWellFormedUnicode(value) ||
    /[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/u.test(value)
  ) {
    throw new Error(`The dependency graph contains an invalid ${field}.`);
  }
}

function isWellFormedUnicode(value) {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
      index += 1;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return false;
    }
  }
  return true;
}

function assertDeclaredDocumentCoordinate(document, org, name, version) {
  if (
    document?.view !== "declared" ||
    document.package?.org !== org ||
    document.package?.name !== name ||
    document.package?.version !== version
  ) {
    throw new Error("The dependency graph response did not match the requested exact package version.");
  }
}

function assertOptionalGraphText(value, field) {
  if (value !== undefined && value !== null && value !== "") assertGraphText(value, field);
}

function assertOptionalBoolean(value, field) {
  if (value !== undefined && typeof value !== "boolean") {
    throw new Error(`The dependency graph contains an invalid ${field}.`);
  }
}

function assertStringList(value, field) {
  if (value === undefined) return;
  if (!Array.isArray(value) || value.length > MAX_EDGE_FEATURES) {
    throw new Error(`The dependency graph contains invalid ${field}.`);
  }
  for (const item of value) assertGraphText(item, field);
}

function assertDependencyKind(value) {
  if (!Object.hasOwn(KIND_LABELS, value)) {
    throw new Error("The dependency graph contains an invalid dependency kind.");
  }
}

if (globalThis.customElements && !customElements.get("zed-dependency-graph")) {
  customElements.define("zed-dependency-graph", ZedDependencyGraph);
}

export {
  ZedDependencyGraph,
  adjacency,
  aggregateQueryNodes,
  assertDeclaredDocumentCoordinate,
  cacheControlDisallowsStorage,
  boundedRenderedNodeSet,
  edgeIdentity,
  edgePairIdentity,
  escapeHtml,
  graphInstanceIdentifiers,
  graphViewUrl,
  isPrereleaseVersion,
  isUnmaintainedNode,
  isGraphDigest,
  isStrongGraphEtag,
  packageDocumentUrl,
  packageExportUrl,
  packagePageUrl,
  parseContentLength,
  parseGraphViewState,
  pathEdgePairs,
  projectionFilename,
  projectionSvgDocument,
  scopeSourceBatches,
};
