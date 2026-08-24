import {
  graphTopologyProfile,
  minimapGeometry,
  recommendGraphLayout,
} from "./dependency-graph-insights-core.js";

const graphModuleUrl =
  typeof document === "undefined"
    ? new URL("../assets/dependency-graph.js", import.meta.url)
    : "/graph-assets/dependency-graph.js";
const { ZedDependencyGraph } = await import(graphModuleUrl);

const SVG_NS = "http://www.w3.org/2000/svg";
const PATCH_MARKER = Symbol.for("zpkg.dependency-graph.visual-search.v1");
const INSTANCE_STATE = Symbol("zpkg dependency graph visual-search state");
const MINIMAP_WIDTH = 196;
const MINIMAP_HEIGHT = 118;
const MINIMAP_NODE_LIMIT = 500;
const MINIMAP_EDGE_LIMIT = 900;

function svgElement(name, attributes = {}) {
  const element = document.createElementNS(SVG_NS, name);
  for (const [key, value] of Object.entries(attributes)) {
    element.setAttribute(key, String(value));
  }
  return element;
}

function topologyEdges(graph) {
  try {
    return typeof graph.filteredEdges === "function"
      ? graph.filteredEdges(false)
      : graph.edges || [];
  } catch {
    return graph.edges || [];
  }
}

function graphState(graph) {
  if (!graph[INSTANCE_STATE]) {
    graph[INSTANCE_STATE] = {
      recommendation: null,
      minimapGeometry: null,
      minimapIds: [],
      minimapRenderFrame: null,
      minimapViewportFrame: null,
    };
  }
  return graph[INSTANCE_STATE];
}

function enhancementMarkup() {
  const intelligence = document.createElement("aside");
  intelligence.className = "dg-visual-search";
  intelligence.dataset.role = "visual-search";
  intelligence.setAttribute("aria-live", "polite");
  intelligence.innerHTML = `
    <p class="dg-visual-search-kicker">Visual search</p>
    <div class="dg-visual-search-heading">
      <strong data-role="recommended-layout">Analyzing…</strong>
      <span>recommended layout</span>
    </div>
    <p class="dg-visual-search-rationale" data-role="layout-rationale"></p>
    <div class="dg-layout-scores" aria-label="Layout suitability scores">
      <span data-score-layout="layered"><i></i><b>Layered</b><em>0</em></span>
      <span data-score-layout="radial"><i></i><b>Radial</b><em>0</em></span>
      <span data-score-layout="force"><i></i><b>Force</b><em>0</em></span>
    </div>
    <button type="button" data-action="apply-recommended-layout">Apply recommendation</button>`;

  const minimap = document.createElement("aside");
  minimap.className = "dg-minimap";
  minimap.dataset.role = "minimap";
  minimap.innerHTML = `
    <div class="dg-minimap-heading">
      <span><b>Overview</b><small>Click to navigate</small></span>
      <button type="button" data-action="fit-minimap" aria-label="Fit dependency graph">Fit</button>
    </div>`;
  const minimapSvg = svgElement("svg", {
    viewBox: `0 0 ${MINIMAP_WIDTH} ${MINIMAP_HEIGHT}`,
    role: "img",
    "aria-label": "Dependency graph overview navigator",
    tabindex: "0",
  });
  minimapSvg.dataset.role = "minimap-svg";
  minimap.appendChild(minimapSvg);
  return { intelligence, minimap, minimapSvg };
}

function ensureGraphEnhancements(graph) {
  const viewport = graph.querySelector?.('[data-role="viewport"]');
  if (!viewport) return null;
  const state = graphState(graph);
  let intelligence = viewport.querySelector('[data-role="visual-search"]');
  let minimap = viewport.querySelector('[data-role="minimap"]');
  let minimapSvg = viewport.querySelector('[data-role="minimap-svg"]');
  if (!intelligence || !minimap || !minimapSvg) {
    const markup = enhancementMarkup();
    intelligence = markup.intelligence;
    minimap = markup.minimap;
    minimapSvg = markup.minimapSvg;
    viewport.append(intelligence, minimap);

    intelligence
      .querySelector('[data-action="apply-recommended-layout"]')
      .addEventListener("click", () => applyRecommendedLayout(graph));
    minimap
      .querySelector('[data-action="fit-minimap"]')
      .addEventListener("click", () => graph.fitGraph?.());
    minimapSvg.addEventListener("pointerdown", (event) => navigateFromMinimap(graph, event));
    minimapSvg.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        graph.fitGraph?.();
      }
    });
  }
  state.intelligence = intelligence;
  state.minimap = minimap;
  state.minimapSvg = minimapSvg;
  graph.dataset.visualSearch = "ready";
  return state;
}

function updateLayoutButtons(graph, recommendation) {
  graph.querySelectorAll?.("[data-layout]").forEach((button) => {
    const layout = button.dataset.layout;
    const recommended = layout === recommendation.recommended;
    button.classList.toggle("is-recommended", recommended);
    button.dataset.layoutScore = String(recommendation.scores[layout] ?? 0);
    const baseTitle = `${layout[0].toUpperCase()}${layout.slice(1)} layout`;
    button.title = recommended
      ? `${baseTitle} — recommended for this topology`
      : `${baseTitle} — suitability ${recommendation.scores[layout] ?? 0}`;
  });
}

function updateVisualSearch(graph) {
  const state = ensureGraphEnhancements(graph);
  if (!state) return;
  if (!graph.nodes?.size) {
    state.intelligence.hidden = true;
    state.minimap.hidden = true;
    return;
  }

  const edges = topologyEdges(graph);
  const profile = graphTopologyProfile(graph.nodes, edges, graph.roots);
  const recommendation = recommendGraphLayout(profile);
  state.profile = profile;
  state.recommendation = recommendation;
  state.intelligence.hidden = false;
  state.minimap.hidden = false;
  graph.dataset.recommendedLayout = recommendation.recommended;

  state.intelligence.querySelector('[data-role="recommended-layout"]').textContent =
    recommendation.recommended[0].toUpperCase() + recommendation.recommended.slice(1);
  state.intelligence.querySelector('[data-role="layout-rationale"]').textContent =
    recommendation.rationale;
  state.intelligence.querySelectorAll("[data-score-layout]").forEach((row) => {
    const score = recommendation.scores[row.dataset.scoreLayout] ?? 0;
    row.style.setProperty("--dg-layout-score", `${score}%`);
    row.querySelector("em").textContent = String(score);
  });
  const apply = state.intelligence.querySelector('[data-action="apply-recommended-layout"]');
  const active = graph.layoutName === recommendation.recommended;
  apply.disabled = active;
  apply.textContent = active ? "Recommended layout active" : "Apply recommendation";
  updateLayoutButtons(graph, recommendation);
  renderMinimap(graph);
}

function applyRecommendedLayout(graph) {
  const state = graphState(graph);
  const layout = state.recommendation?.recommended;
  if (!layout || graph.layoutName === layout) return;
  graph.layoutName = layout;
  try {
    localStorage.setItem("zpkg.graph.layout", layout);
  } catch {
    // Recommendation remains usable when browser storage is unavailable.
  }
  graph.querySelectorAll?.("[data-layout]").forEach((button) =>
    button.setAttribute("aria-pressed", String(button.dataset.layout === layout))
  );
  graph.applyLayout?.(true);
  graph.syncViewUrl?.();
}

function sampledMinimapIds(graph) {
  const rendered = [...(graph.renderedNodeElements?.keys?.() || [])];
  const prioritized = [];
  if (graph.selectedId && rendered.includes(graph.selectedId)) prioritized.push(graph.selectedId);
  for (const root of graph.roots || []) {
    if (rendered.includes(root) && !prioritized.includes(root)) prioritized.push(root);
  }
  const degrees = new Map(rendered.map((id) => [id, 0]));
  for (const edge of topologyEdges(graph)) {
    if (degrees.has(edge.from)) degrees.set(edge.from, degrees.get(edge.from) + 1);
    if (degrees.has(edge.to)) degrees.set(edge.to, degrees.get(edge.to) + 1);
  }
  const remaining = rendered
    .filter((id) => !prioritized.includes(id))
    .sort((left, right) =>
      (degrees.get(right) || 0) - (degrees.get(left) || 0) || left.localeCompare(right)
    );
  return [...prioritized, ...remaining].slice(0, MINIMAP_NODE_LIMIT);
}

function renderMinimap(graph) {
  const state = ensureGraphEnhancements(graph);
  if (!state || !graph.nodes?.size || !graph.positions?.size) return;
  const ids = sampledMinimapIds(graph);
  const idSet = new Set(ids);
  const positions = new Map(
    ids
      .map((id) => [id, graph.positions.get(id)])
      .filter(([, position]) => Number.isFinite(position?.x) && Number.isFinite(position?.y))
  );
  if (!positions.size) {
    state.minimap.hidden = true;
    return;
  }
  const geometry = minimapGeometry(
    positions,
    graph.transform,
    {
      width: graph.viewport?.clientWidth || 1,
      height: graph.viewport?.clientHeight || 1,
    },
    { width: MINIMAP_WIDTH, height: MINIMAP_HEIGHT, padding: 10 }
  );
  state.minimapGeometry = geometry;
  state.minimapIds = ids;

  const fragment = document.createDocumentFragment();
  const edgeLayer = svgElement("g", { class: "dg-minimap-edges" });
  const nodeLayer = svgElement("g", { class: "dg-minimap-nodes" });
  for (const edge of topologyEdges(graph)
    .filter((edge) => idSet.has(edge.from) && idSet.has(edge.to))
    .slice(0, MINIMAP_EDGE_LIMIT)) {
    const from = geometry.points.get(edge.from);
    const to = geometry.points.get(edge.to);
    if (!from || !to) continue;
    edgeLayer.appendChild(
      svgElement("line", {
        x1: from.x,
        y1: from.y,
        x2: to.x,
        y2: to.y,
        class: `dg-minimap-edge dg-kind-${edge.kind || "runtime"}`,
      })
    );
  }
  for (const id of ids) {
    const point = geometry.points.get(id);
    if (!point) continue;
    const classes = ["dg-minimap-node"];
    if (graph.roots?.has(id)) classes.push("is-root");
    if (id === graph.selectedId) classes.push("is-selected");
    nodeLayer.appendChild(
      svgElement("circle", {
        cx: point.x,
        cy: point.y,
        r: id === graph.selectedId ? 3.2 : graph.roots?.has(id) ? 2.6 : 1.8,
        class: classes.join(" "),
      })
    );
  }
  const viewportRect = svgElement("rect", {
    class: "dg-minimap-viewport",
    x: geometry.viewport.x,
    y: geometry.viewport.y,
    width: geometry.viewport.width,
    height: geometry.viewport.height,
    rx: 3,
  });
  fragment.append(edgeLayer, nodeLayer, viewportRect);
  state.minimapSvg.replaceChildren(fragment);
  state.viewportRect = viewportRect;
  state.minimapSvg.setAttribute(
    "aria-label",
    `Dependency graph overview with ${positions.size} packages; click to recenter the main graph`
  );
}

function updateMinimapViewport(graph) {
  const state = graphState(graph);
  if (!state.minimapSvg || !state.minimapIds?.length || !state.viewportRect) return;
  const positions = new Map(
    state.minimapIds
      .map((id) => [id, graph.positions.get(id)])
      .filter(([, position]) => Number.isFinite(position?.x) && Number.isFinite(position?.y))
  );
  const geometry = minimapGeometry(
    positions,
    graph.transform,
    {
      width: graph.viewport?.clientWidth || 1,
      height: graph.viewport?.clientHeight || 1,
    },
    { width: MINIMAP_WIDTH, height: MINIMAP_HEIGHT, padding: 10 }
  );
  state.minimapGeometry = geometry;
  for (const [attribute, value] of Object.entries(geometry.viewport)) {
    state.viewportRect.setAttribute(attribute, String(value));
  }
}

function queueMinimapViewportUpdate(graph) {
  const state = graphState(graph);
  if (state.minimapViewportFrame !== null) return;
  state.minimapViewportFrame = requestAnimationFrame(() => {
    state.minimapViewportFrame = null;
    updateMinimapViewport(graph);
  });
}

function queueMinimapRender(graph) {
  const state = graphState(graph);
  if (state.minimapRenderFrame !== null) return;
  state.minimapRenderFrame = requestAnimationFrame(() => {
    state.minimapRenderFrame = null;
    renderMinimap(graph);
  });
}

function navigateFromMinimap(graph, event) {
  const state = graphState(graph);
  const geometry = state.minimapGeometry;
  const svg = state.minimapSvg;
  if (!geometry || !svg || !graph.viewport) return;
  event.preventDefault();
  event.stopPropagation();
  const bounds = svg.getBoundingClientRect();
  if (!bounds.width || !bounds.height) return;
  const minimapX = ((event.clientX - bounds.left) / bounds.width) * MINIMAP_WIDTH;
  const minimapY = ((event.clientY - bounds.top) / bounds.height) * MINIMAP_HEIGHT;
  const world = geometry.worldAt(minimapX, minimapY);
  const zoom = Math.max(0.08, Number(graph.transform?.k) || 1);
  graph.transform = {
    x: graph.viewport.clientWidth / 2 - world.x * zoom,
    y: graph.viewport.clientHeight / 2 - world.y * zoom,
    k: zoom,
  };
  graph.updateTransform?.();
}

function wrapAfter(prototype, methodName, after) {
  const original = prototype[methodName];
  if (typeof original !== "function") return;
  prototype[methodName] = function wrappedGraphInsightMethod(...args) {
    const result = original.apply(this, args);
    after(this, args, result);
    return result;
  };
}

function installPrototypeEnhancements() {
  const prototype = ZedDependencyGraph.prototype;
  if (prototype[PATCH_MARKER]) return;
  Object.defineProperty(prototype, PATCH_MARKER, { value: true });
  wrapAfter(prototype, "connectedCallback", (graph) => {
    ensureGraphEnhancements(graph);
    queueMicrotask(() => updateVisualSearch(graph));
  });
  wrapAfter(prototype, "renderShell", (graph) => ensureGraphEnhancements(graph));
  wrapAfter(prototype, "renderGraph", (graph) => updateVisualSearch(graph));
  wrapAfter(prototype, "updateTransform", (graph) => queueMinimapViewportUpdate(graph));
  wrapAfter(prototype, "updateDraggedNode", (graph) => queueMinimapRender(graph));
  wrapAfter(prototype, "clearGraph", (graph) => {
    const state = graphState(graph);
    if (state.intelligence) state.intelligence.hidden = true;
    if (state.minimap) state.minimap.hidden = true;
  });
  wrapAfter(prototype, "disconnectedCallback", (graph) => {
    const state = graphState(graph);
    if (state.minimapRenderFrame !== null) cancelAnimationFrame(state.minimapRenderFrame);
    if (state.minimapViewportFrame !== null) cancelAnimationFrame(state.minimapViewportFrame);
    state.minimapRenderFrame = null;
    state.minimapViewportFrame = null;
  });
}

function installExistingGraphs() {
  if (typeof document === "undefined") return;
  document.querySelectorAll("zed-dependency-graph").forEach((graph) => {
    ensureGraphEnhancements(graph);
    updateVisualSearch(graph);
  });
}

installPrototypeEnhancements();
if (typeof document !== "undefined") {
  installExistingGraphs();
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (!(node instanceof Element)) continue;
        if (node.matches("zed-dependency-graph")) {
          ensureGraphEnhancements(node);
          updateVisualSearch(node);
        }
        node.querySelectorAll?.("zed-dependency-graph").forEach((graph) => {
          ensureGraphEnhancements(graph);
          updateVisualSearch(graph);
        });
      }
    }
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });
}

export {
  applyRecommendedLayout,
  ensureGraphEnhancements,
  renderMinimap,
  updateVisualSearch,
};
