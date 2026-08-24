const FORCE_LAYOUT_NODE_LIMIT = 260;
const DEFAULT_MINIMAP_WIDTH = 196;
const DEFAULT_MINIMAP_HEIGHT = 118;
const DEFAULT_MINIMAP_PADDING = 10;

function finiteNumber(value, fallback = 0) {
  return Number.isFinite(value) ? value : fallback;
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function nodeIdsFrom(nodes) {
  if (nodes instanceof Map) return [...nodes.keys()];
  if (nodes instanceof Set) return [...nodes];
  if (Array.isArray(nodes)) {
    return nodes.map((node) => (typeof node === "string" ? node : node?.id)).filter(Boolean);
  }
  return [];
}

function rootIdsFrom(roots, ids) {
  const known = new Set(ids);
  const values = roots instanceof Set || Array.isArray(roots) ? [...roots] : [];
  return values.filter((id) => known.has(id));
}

function graphTopologyProfile(nodes, edges = [], roots = new Set()) {
  const ids = [...new Set(nodeIdsFrom(nodes))];
  const idSet = new Set(ids);
  const safeEdges = Array.isArray(edges)
    ? edges.filter(
        (edge) => edge && idSet.has(edge.from) && idSet.has(edge.to)
      )
    : [];
  const incoming = new Map(ids.map((id) => [id, 0]));
  const outgoing = new Map(ids.map((id) => [id, []]));
  const undirected = new Map(ids.map((id) => [id, new Set()]));
  for (const edge of safeEdges) {
    incoming.set(edge.to, (incoming.get(edge.to) || 0) + 1);
    outgoing.get(edge.from).push(edge.to);
    undirected.get(edge.from).add(edge.to);
    undirected.get(edge.to).add(edge.from);
  }

  let rootIds = rootIdsFrom(roots, ids);
  if (!rootIds.length) rootIds = ids.filter((id) => (incoming.get(id) || 0) === 0);
  if (!rootIds.length && ids.length) rootIds = [ids[0]];

  const levels = new Map();
  const queue = [];
  for (const root of rootIds) {
    if (levels.has(root)) continue;
    levels.set(root, 0);
    queue.push(root);
  }
  for (let cursor = 0; cursor < queue.length; cursor += 1) {
    const id = queue[cursor];
    const nextLevel = (levels.get(id) || 0) + 1;
    for (const next of outgoing.get(id) || []) {
      if (levels.has(next)) continue;
      levels.set(next, nextLevel);
      queue.push(next);
    }
  }
  let fallbackLevel = levels.size ? Math.max(...levels.values()) + 1 : 0;
  for (const id of ids) {
    if (levels.has(id)) continue;
    levels.set(id, fallbackLevel);
    fallbackLevel += 1;
  }

  const widths = new Map();
  for (const level of levels.values()) widths.set(level, (widths.get(level) || 0) + 1);

  const remainingIncoming = new Map(incoming);
  const acyclicQueue = ids.filter((id) => (remainingIncoming.get(id) || 0) === 0);
  let processed = 0;
  for (let cursor = 0; cursor < acyclicQueue.length; cursor += 1) {
    const id = acyclicQueue[cursor];
    processed += 1;
    for (const next of outgoing.get(id) || []) {
      const nextIncoming = (remainingIncoming.get(next) || 0) - 1;
      remainingIncoming.set(next, nextIncoming);
      if (nextIncoming === 0) acyclicQueue.push(next);
    }
  }

  let componentCount = 0;
  const visited = new Set();
  for (const start of ids) {
    if (visited.has(start)) continue;
    componentCount += 1;
    const componentQueue = [start];
    visited.add(start);
    for (let cursor = 0; cursor < componentQueue.length; cursor += 1) {
      for (const next of undirected.get(componentQueue[cursor]) || []) {
        if (visited.has(next)) continue;
        visited.add(next);
        componentQueue.push(next);
      }
    }
  }

  const degrees = ids.map(
    (id) => (outgoing.get(id)?.length || 0) + (incoming.get(id) || 0)
  );
  const nodeCount = ids.length;
  const edgeCount = safeEdges.length;
  const maxDegree = degrees.length ? Math.max(...degrees) : 0;
  const nonLeafCount = ids.filter((id) => (outgoing.get(id)?.length || 0) > 0).length;
  const maxDepth = levels.size ? Math.max(...levels.values()) : 0;
  const maxWidth = widths.size ? Math.max(...widths.values()) : 0;
  const possibleEdges = Math.max(1, nodeCount * Math.max(1, nodeCount - 1));

  return Object.freeze({
    nodeCount,
    edgeCount,
    rootCount: rootIds.length,
    componentCount,
    maxDepth,
    maxWidth,
    maxDegree,
    averageDegree: nodeCount ? (edgeCount * 2) / nodeCount : 0,
    averageBranching: nonLeafCount ? edgeCount / nonLeafCount : 0,
    density: edgeCount / possibleEdges,
    hubRatio: nodeCount > 1 ? maxDegree / (nodeCount - 1) : 0,
    cycleRatio: nodeCount ? Math.max(0, nodeCount - processed) / nodeCount : 0,
  });
}

function scoreClamp(value) {
  return Math.round(clamp(value, 0, 100));
}

function recommendGraphLayout(profile) {
  const nodeCount = finiteNumber(profile?.nodeCount);
  const maxDepth = finiteNumber(profile?.maxDepth);
  const rootCount = finiteNumber(profile?.rootCount);
  const componentCount = finiteNumber(profile?.componentCount);
  const density = finiteNumber(profile?.density);
  const hubRatio = finiteNumber(profile?.hubRatio);
  const cycleRatio = finiteNumber(profile?.cycleRatio);

  if (nodeCount <= 2) {
    return Object.freeze({
      recommended: "layered",
      scores: Object.freeze({ layered: 100, radial: 46, force: 28 }),
      rationale: "A tiny topology reads most clearly as a direct dependency flow.",
    });
  }

  let layered = 35;
  layered += Math.min(36, maxDepth * 7);
  layered += cycleRatio === 0 ? 20 : -cycleRatio * 35;
  layered += rootCount <= 2 ? 8 : 0;
  layered += nodeCount > 80 ? 12 : 0;
  layered += componentCount > 1 ? 5 : 0;

  let radial = 28;
  radial += hubRatio * 45;
  radial += rootCount > 1 ? Math.min(18, rootCount * 3) : 4;
  radial += maxDepth <= 4 ? 15 : 0;
  radial -= cycleRatio * 10;
  radial -= nodeCount > 320 ? 20 : 0;

  let force = 20;
  force += cycleRatio * 65;
  force += Math.min(1, density * 8) * 35;
  force += componentCount > 1 ? 10 : 0;
  force += nodeCount <= 80 ? 18 : nodeCount <= 160 ? 8 : -20;
  if (nodeCount > FORCE_LAYOUT_NODE_LIMIT) force = -1;

  const scores = Object.freeze({
    layered: scoreClamp(layered),
    radial: scoreClamp(radial),
    force: force < 0 ? 0 : scoreClamp(force),
  });
  const recommended = ["layered", "radial", "force"].sort(
    (left, right) => scores[right] - scores[left]
  )[0];
  const rationale = {
    layered:
      "A deep, mostly directional topology benefits from stable layers and clear dependency flow.",
    radial:
      "A hub-and-spoke topology benefits from a compact radial overview around its central packages.",
    force:
      "A compact cyclic or dense topology benefits from a force layout that exposes local clusters.",
  }[recommended];

  return Object.freeze({ recommended, scores, rationale });
}

function positionEntriesFrom(positions) {
  if (positions instanceof Map) return [...positions.entries()];
  if (Array.isArray(positions)) {
    return positions
      .map((entry, index) => {
        if (Array.isArray(entry)) return entry;
        return [entry?.id || String(index), entry];
      })
      .filter(([, position]) => position);
  }
  return [];
}

function minimapGeometry(
  positions,
  transform = { x: 0, y: 0, k: 1 },
  viewport = { width: 1, height: 1 },
  options = {}
) {
  const entries = positionEntriesFrom(positions).filter(
    ([, position]) => Number.isFinite(position?.x) && Number.isFinite(position?.y)
  );
  const width = Math.max(1, finiteNumber(options.width, DEFAULT_MINIMAP_WIDTH));
  const height = Math.max(1, finiteNumber(options.height, DEFAULT_MINIMAP_HEIGHT));
  const padding = clamp(
    finiteNumber(options.padding, DEFAULT_MINIMAP_PADDING),
    0,
    Math.min(width, height) / 3
  );
  if (!entries.length) {
    return Object.freeze({
      width,
      height,
      padding,
      points: new Map(),
      viewport: Object.freeze({ x: padding, y: padding, width: 0, height: 0 }),
      worldAt: () => Object.freeze({ x: 0, y: 0 }),
    });
  }

  const xs = entries.map(([, position]) => position.x);
  const ys = entries.map(([, position]) => position.y);
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minY = Math.min(...ys);
  const maxY = Math.max(...ys);
  const worldWidth = Math.max(1, maxX - minX);
  const worldHeight = Math.max(1, maxY - minY);
  const availableWidth = Math.max(1, width - padding * 2);
  const availableHeight = Math.max(1, height - padding * 2);
  const scale = Math.min(availableWidth / worldWidth, availableHeight / worldHeight);
  const renderedWidth = worldWidth * scale;
  const renderedHeight = worldHeight * scale;
  const offsetX = (width - renderedWidth) / 2;
  const offsetY = (height - renderedHeight) / 2;
  const mapX = (x) => offsetX + (x - minX) * scale;
  const mapY = (y) => offsetY + (y - minY) * scale;
  const points = new Map(
    entries.map(([id, position]) => [id, Object.freeze({ x: mapX(position.x), y: mapY(position.y) })])
  );

  const zoom = Math.max(0.0001, finiteNumber(transform.k, 1));
  const viewportWidth = Math.max(1, finiteNumber(viewport.width, 1));
  const viewportHeight = Math.max(1, finiteNumber(viewport.height, 1));
  const worldViewport = {
    x: -finiteNumber(transform.x) / zoom,
    y: -finiteNumber(transform.y) / zoom,
    width: viewportWidth / zoom,
    height: viewportHeight / zoom,
  };
  const rawViewport = {
    x: mapX(worldViewport.x),
    y: mapY(worldViewport.y),
    width: worldViewport.width * scale,
    height: worldViewport.height * scale,
  };
  const minimapViewport = Object.freeze({
    x: clamp(rawViewport.x, 0, width),
    y: clamp(rawViewport.y, 0, height),
    width: clamp(rawViewport.width, 1, width),
    height: clamp(rawViewport.height, 1, height),
  });

  return Object.freeze({
    width,
    height,
    padding,
    points,
    viewport: minimapViewport,
    bounds: Object.freeze({ minX, maxX, minY, maxY, scale, offsetX, offsetY }),
    worldAt: (minimapX, minimapY) =>
      Object.freeze({
        x: minX + (finiteNumber(minimapX) - offsetX) / scale,
        y: minY + (finiteNumber(minimapY) - offsetY) / scale,
      }),
  });
}

export {
  FORCE_LAYOUT_NODE_LIMIT,
  graphTopologyProfile,
  minimapGeometry,
  recommendGraphLayout,
};
