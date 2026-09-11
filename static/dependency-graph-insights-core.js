const FORCE_LAYOUT_NODE_LIMIT = 260;
const DEFAULT_MINIMAP_WIDTH = 196;
const DEFAULT_MINIMAP_HEIGHT = 118;
const DEFAULT_MINIMAP_PADDING = 10;
const PROFILE_MAX_NODES = 3000;
const PROFILE_MAX_EDGES = 12000;
const PROFILE_MAX_TEXT_BYTES = 2048;
const UTF8_ENCODER = new TextEncoder();

function finiteNumber(value, fallback = 0) {
  return Number.isFinite(value) ? value : fallback;
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function nodeIdsFrom(nodes) {
  let values;
  if (nodes instanceof Map || nodes instanceof Set) {
    if (nodes.size > PROFILE_MAX_NODES) {
      throw new RangeError(`topology profile exceeds the ${PROFILE_MAX_NODES}-node contract limit`);
    }
    values = nodes instanceof Map ? [...nodes.keys()] : [...nodes];
  }
  if (Array.isArray(nodes)) {
    if (nodes.length > PROFILE_MAX_NODES) {
      throw new RangeError(`topology profile exceeds the ${PROFILE_MAX_NODES}-node contract limit`);
    }
    values = nodes.map((node) => (typeof node === "string" ? node : node?.id));
  }
  if (!values) return [];
  if (values.length > PROFILE_MAX_NODES) {
    throw new RangeError(`topology profile exceeds the ${PROFILE_MAX_NODES}-node contract limit`);
  }
  if (!values.every(validGraphId)) {
    throw new TypeError("topology profile contains an invalid node identifier");
  }
  return values;
}

function validGraphId(value) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    UTF8_ENCODER.encode(value).byteLength <= PROFILE_MAX_TEXT_BYTES &&
    !/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/u.test(value)
  );
}

function rootIdsFrom(roots, ids) {
  const known = new Set(ids);
  if (
    (roots instanceof Set && roots.size > PROFILE_MAX_NODES) ||
    (Array.isArray(roots) && roots.length > PROFILE_MAX_NODES)
  ) {
    throw new RangeError(`topology profile exceeds the ${PROFILE_MAX_NODES}-root contract limit`);
  }
  const values = roots instanceof Set || Array.isArray(roots) ? [...roots] : [];
  if (values.length > PROFILE_MAX_NODES) {
    throw new RangeError(`topology profile exceeds the ${PROFILE_MAX_NODES}-root contract limit`);
  }
  if (!values.every((id) => validGraphId(id) && known.has(id))) {
    throw new TypeError("topology profile contains an invalid or unknown root identifier");
  }
  return [...new Set(values)];
}

function compareGraphIds(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function sortedNumbers(values) {
  return [...values].sort((left, right) => left - right);
}

function stronglyConnectedComponents(outgoing, incoming) {
  const seen = new Uint8Array(outgoing.length);
  const order = [];
  for (let start = 0; start < outgoing.length; start += 1) {
    if (seen[start]) continue;
    seen[start] = 1;
    const stack = [{ node: start, cursor: 0 }];
    while (stack.length) {
      const frame = stack[stack.length - 1];
      if (frame.cursor < outgoing[frame.node].length) {
        const next = outgoing[frame.node][frame.cursor];
        frame.cursor += 1;
        if (!seen[next]) {
          seen[next] = 1;
          stack.push({ node: next, cursor: 0 });
        }
      } else {
        order.push(frame.node);
        stack.pop();
      }
    }
  }

  seen.fill(0);
  const components = [];
  while (order.length) {
    const start = order.pop();
    if (seen[start]) continue;
    seen[start] = 1;
    const component = [];
    const stack = [start];
    while (stack.length) {
      const node = stack.pop();
      component.push(node);
      for (let index = incoming[node].length - 1; index >= 0; index -= 1) {
        const next = incoming[node][index];
        if (!seen[next]) {
          seen[next] = 1;
          stack.push(next);
        }
      }
    }
    component.sort((left, right) => left - right);
    components.push(component);
  }
  components.sort((left, right) => left[0] - right[0]);
  return components;
}

function weakComponentCount(undirected) {
  const seen = new Uint8Array(undirected.length);
  let count = 0;
  for (let start = 0; start < undirected.length; start += 1) {
    if (seen[start]) continue;
    count += 1;
    seen[start] = 1;
    const queue = [start];
    for (let cursor = 0; cursor < queue.length; cursor += 1) {
      for (const next of undirected[queue[cursor]]) {
        if (!seen[next]) {
          seen[next] = 1;
          queue.push(next);
        }
      }
    }
  }
  return count;
}

function condensationProfile(components, outgoing) {
  const owner = new Uint32Array(outgoing.length);
  components.forEach((component, componentId) => {
    for (const node of component) owner[node] = componentId;
  });
  const graph = components.map(() => new Set());
  const indegree = new Uint32Array(components.length);
  for (let from = 0; from < outgoing.length; from += 1) {
    for (const to of outgoing[from]) {
      const source = owner[from];
      const target = owner[to];
      if (source !== target && !graph[source].has(target)) {
        graph[source].add(target);
        indegree[target] += 1;
      }
    }
  }
  const depth = new Uint32Array(components.length);
  const widths = new Map();
  const queue = [];
  for (let component = 0; component < components.length; component += 1) {
    if (indegree[component] === 0) queue.push(component);
  }
  for (let cursor = 0; cursor < queue.length; cursor += 1) {
    const component = queue[cursor];
    widths.set(depth[component], (widths.get(depth[component]) || 0) + components[component].length);
    for (const next of sortedNumbers(graph[component])) {
      depth[next] = Math.max(depth[next], depth[component] + 1);
      indegree[next] -= 1;
      if (indegree[next] === 0) queue.push(next);
    }
  }
  return {
    maxDepth: depth.length ? Math.max(...depth) : 0,
    maxWidth: widths.size ? Math.max(...widths.values()) : 0,
  };
}

function graphTopologyProfile(nodes, edges = [], roots = new Set()) {
  const ids = nodeIdsFrom(nodes).sort(compareGraphIds);
  if (new Set(ids).size !== ids.length) {
    throw new TypeError("topology profile contains duplicate node identifiers");
  }
  const indexById = new Map(ids.map((id, index) => [id, index]));
  const safeEdges = [];
  const edgeKeys = new Set();
  if (!Array.isArray(edges)) {
    throw new TypeError("topology profile relationships must be an array");
  }
  if (edges.length > PROFILE_MAX_EDGES) {
    throw new RangeError(`topology profile exceeds the ${PROFILE_MAX_EDGES}-edge contract limit`);
  }
  for (const edge of edges) {
    const kind = edge?.kind === undefined || edge.kind === "" ? "runtime" : edge.kind;
    if (
      !edge ||
      !indexById.has(edge.from) ||
      !indexById.has(edge.to) ||
      !validGraphId(kind) ||
      (edge.optional !== undefined && typeof edge.optional !== "boolean")
    ) {
      throw new TypeError("topology profile contains an invalid relationship");
    }
    const key = JSON.stringify([edge.from, edge.to, kind, edge.optional === true]);
    if (edgeKeys.has(key)) continue;
    edgeKeys.add(key);
    safeEdges.push(edge);
  }
  const outgoingSets = ids.map(() => new Set());
  const incomingSets = ids.map(() => new Set());
  const undirected = ids.map(() => new Set());
  const selfLoops = new Set();
  for (const edge of safeEdges) {
    const from = indexById.get(edge.from);
    const to = indexById.get(edge.to);
    outgoingSets[from].add(to);
    incomingSets[to].add(from);
    undirected[from].add(to);
    undirected[to].add(from);
    if (from === to) selfLoops.add(from);
  }
  const outgoing = outgoingSets.map(sortedNumbers);
  const incoming = incomingSets.map(sortedNumbers);
  let rootIds = rootIdsFrom(roots, ids);
  if (!rootIds.length) rootIds = ids.filter((id) => incoming[indexById.get(id)].length === 0);
  if (!rootIds.length && ids.length) rootIds = [ids[0]];
  const components = stronglyConnectedComponents(outgoing, incoming);
  const condensation = condensationProfile(components, outgoing);
  const cyclicNodeCount = components
    .filter((component) => component.length > 1 || selfLoops.has(component[0]))
    .reduce((total, component) => total + component.length, 0);
  const degrees = ids.map((_, index) => outgoing[index].length + incoming[index].length);
  const nodeCount = ids.length;
  const edgeCount = safeEdges.length;
  const pairEdgeCount = outgoing.reduce((total, neighbors) => total + neighbors.length, 0);
  const maxDegree = degrees.length ? Math.max(...degrees) : 0;
  const nonLeafCount = outgoing.filter((neighbors) => neighbors.length > 0).length;
  const possibleEdges = Math.max(1, nodeCount * Math.max(1, nodeCount - 1));

  return Object.freeze({
    nodeCount,
    edgeCount,
    rootCount: rootIds.length,
    componentCount: weakComponentCount(undirected),
    stronglyConnectedComponentCount: components.length,
    cyclicNodeCount,
    maxDepth: condensation.maxDepth,
    maxWidth: condensation.maxWidth,
    maxDegree,
    averageDegree: nodeCount ? (edgeCount * 2) / nodeCount : 0,
    averageBranching: nonLeafCount ? edgeCount / nonLeafCount : 0,
    density: pairEdgeCount / possibleEdges,
    hubRatio: nodeCount > 1 ? maxDegree / (nodeCount - 1) : 0,
    cycleRatio: nodeCount ? cyclicNodeCount / nodeCount : 0,
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
  const minimapViewportWidth = clamp(rawViewport.width, 1, width);
  const minimapViewportHeight = clamp(rawViewport.height, 1, height);
  const minimapViewport = Object.freeze({
    x: clamp(rawViewport.x, 0, Math.max(0, width - minimapViewportWidth)),
    y: clamp(rawViewport.y, 0, Math.max(0, height - minimapViewportHeight)),
    width: minimapViewportWidth,
    height: minimapViewportHeight,
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
  PROFILE_MAX_EDGES,
  PROFILE_MAX_NODES,
  PROFILE_MAX_TEXT_BYTES,
  graphTopologyProfile,
  minimapGeometry,
  recommendGraphLayout,
};
