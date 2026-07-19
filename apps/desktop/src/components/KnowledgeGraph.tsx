import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type SimulationLinkDatum,
  type SimulationNodeDatum
} from "d3-force";
import { useEffect, useMemo, useRef, useState } from "react";
import type { GraphEdgeView, GraphNodeView, GraphStateView } from "../tauri";

type KnowledgeGraphProps = {
  graph: GraphStateView;
};

type PositionedNode = GraphNodeView &
  SimulationNodeDatum & {
    x: number;
    y: number;
    degree: number;
    radius: number;
  };

type SimulationEdge = GraphEdgeView & SimulationLinkDatum<PositionedNode>;

type PositionedEdge = GraphEdgeView & {
  source: PositionedNode;
  target: PositionedNode;
};

const WIDTH = 900;
const HEIGHT = 420;
const MAX_VISIBLE_NODES = 56;
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

function motionSeed(value: string) {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0) / 0xffffffff;
}

function nodeColor(kind: string) {
  switch (kind) {
    case "file":
      return "#595959";
    case "symbol":
      return "#737373";
    case "tool":
      return "#666666";
    case "decision":
      return "#888888";
    case "claim":
      return "#7b7b7b";
    default:
      return "#808080";
  }
}

function shortNodeLabel(label: string) {
  const trimmed = label.trim();
  return trimmed.length > 26 ? `${trimmed.slice(0, 25)}…` : trimmed;
}

function layoutGraph(nodes: GraphNodeView[], edges: GraphEdgeView[]) {
  const allDegrees = new Map<string, number>();
  edges.forEach((edge) => {
    allDegrees.set(edge.from, (allDegrees.get(edge.from) ?? 0) + 1);
    allDegrees.set(edge.to, (allDegrees.get(edge.to) ?? 0) + 1);
  });

  const visible = [...nodes]
    .sort(
      (left, right) =>
        Number(right.focused) - Number(left.focused) ||
        (allDegrees.get(right.id) ?? 0) - (allDegrees.get(left.id) ?? 0) ||
        left.label.localeCompare(right.label)
    )
    .slice(0, MAX_VISIBLE_NODES);
  const visibleIds = new Set(visible.map((node) => node.id));
  const visibleEdges = edges.filter(
    (edge) => visibleIds.has(edge.from) && visibleIds.has(edge.to)
  );
  const degrees = new Map<string, number>();
  visibleEdges.forEach((edge) => {
    degrees.set(edge.from, (degrees.get(edge.from) ?? 0) + 1);
    degrees.set(edge.to, (degrees.get(edge.to) ?? 0) + 1);
  });

  const positioned: PositionedNode[] = visible.map((node, index) => {
    const degree = degrees.get(node.id) ?? 0;
    const angle = index * GOLDEN_ANGLE;
    const initialRadius = 28 + Math.sqrt(index + 1) * 22;
    return {
      ...node,
      degree,
      radius: Math.min(6.6, 2.7 + Math.sqrt(degree + 1) * 0.65 + (node.focused ? 0.7 : 0)),
      x: WIDTH / 2 + Math.cos(angle) * initialRadius,
      y: HEIGHT / 2 + Math.sin(angle) * initialRadius * 0.68
    };
  });
  const simulationEdges: SimulationEdge[] = visibleEdges.map((edge) => ({
    ...edge,
    source: edge.from,
    target: edge.to
  }));

  const simulation = forceSimulation(positioned)
    .force(
      "link",
      forceLink<PositionedNode, SimulationEdge>(simulationEdges)
        .id((node) => node.id)
        .distance((edge) => (edge.kind === "related_to" ? 72 : 58))
        .strength(0.38)
    )
    .force(
      "charge",
      forceManyBody<PositionedNode>()
        .strength((node) => -42 - node.radius * 5)
        .distanceMax(220)
    )
    .force(
      "collision",
      forceCollide<PositionedNode>()
        .radius((node) => node.radius + 8)
        .iterations(2)
    )
    .force("center", forceCenter(WIDTH / 2, HEIGHT / 2))
    .force("x", forceX<PositionedNode>(WIDTH / 2).strength(0.025))
    .force("y", forceY<PositionedNode>(HEIGHT / 2).strength(0.04))
    .stop();

  for (let index = 0; index < 220; index += 1) simulation.tick();
  simulation.stop();

  positioned.forEach((node) => {
    node.x = Math.min(WIDTH - 44, Math.max(44, node.x));
    node.y = Math.min(HEIGHT - 34, Math.max(34, node.y));
  });

  const positionedEdges = simulationEdges.flatMap<PositionedEdge>((edge) => {
    if (typeof edge.source !== "object" || typeof edge.target !== "object") return [];
    return [
      {
        id: edge.id,
        from: edge.from,
        to: edge.to,
        kind: edge.kind,
        source: edge.source,
        target: edge.target
      }
    ];
  });

  return { nodes: positioned, edges: positionedEdges };
}

export function KnowledgeGraph({ graph }: KnowledgeGraphProps) {
  const sceneRef = useRef<SVGGElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const layout = useMemo(
    () => layoutGraph(graph.nodes, graph.edges.filter((edge) => edge.from !== edge.to)),
    [graph.edges, graph.nodes]
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const activeId = hoveredId ?? selectedId;
  const selected = layout.nodes.find((node) => node.id === selectedId) ?? null;
  const activeNodeIds = useMemo(() => {
    const ids = new Set<string>();
    if (!activeId) return ids;
    ids.add(activeId);
    layout.edges.forEach((edge) => {
      if (edge.from === activeId) ids.add(edge.to);
      if (edge.to === activeId) ids.add(edge.from);
    });
    return ids;
  }, [activeId, layout.edges]);

  useEffect(() => {
    const scene = sceneRef.current;
    const canvas = canvasRef.current;
    if (
      !scene ||
      !canvas ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      return;
    }
    const nodeElements = Array.from(
      scene.querySelectorAll<SVGGElement>(".knowledge-graph-node")
    );
    const edgeElements = Array.from(
      scene.querySelectorAll<SVGLineElement>(".knowledge-graph-edges line")
    );
    const nodeIndexes = new Map(layout.nodes.map((node, index) => [node.id, index]));
    const edgeIndexes = layout.edges.map((edge) => ({
      source: nodeIndexes.get(edge.from),
      target: nodeIndexes.get(edge.to)
    }));
    const motion = layout.nodes.map((node) => {
      const seed = motionSeed(node.id);
      return {
        phase: seed * Math.PI * 2,
        amplitudeX: 1.2 + seed * 1.6,
        amplitudeY: 1.1 + ((seed * 7.13) % 1) * 1.5,
        speed: 0.17 + ((seed * 3.71) % 1) * 0.09
      };
    });
    const positions = layout.nodes.map((node) => ({ x: node.x, y: node.y }));
    let elapsedMs = 0;
    let previousTimestamp = 0;
    let previousFrame = 0;
    let animationFrame = 0;
    let canvasVisible = !("IntersectionObserver" in window);
    let documentVisible = document.visibilityState !== "hidden";

    const update = (timestamp: number) => {
      if (!canvasVisible || !documentVisible) {
        animationFrame = 0;
        previousTimestamp = 0;
        return;
      }
      if (previousTimestamp > 0) elapsedMs += timestamp - previousTimestamp;
      previousTimestamp = timestamp;
      if (timestamp - previousFrame >= 32) {
        previousFrame = timestamp;
        const elapsed = elapsedMs / 1000;
        layout.nodes.forEach((node, index) => {
          const drift = motion[index];
          const x = node.x + Math.sin(elapsed * drift.speed + drift.phase) * drift.amplitudeX;
          const y =
            node.y +
            Math.cos(elapsed * drift.speed * 0.83 + drift.phase * 1.17) * drift.amplitudeY;
          positions[index] = { x, y };
          nodeElements[index]?.setAttribute("transform", `translate(${x} ${y})`);
        });
        edgeIndexes.forEach((edge, index) => {
          if (edge.source === undefined || edge.target === undefined) return;
          const source = positions[edge.source];
          const target = positions[edge.target];
          const line = edgeElements[index];
          if (!line) return;
          line.setAttribute("x1", String(source.x));
          line.setAttribute("y1", String(source.y));
          line.setAttribute("x2", String(target.x));
          line.setAttribute("y2", String(target.y));
        });
      }
      animationFrame = window.requestAnimationFrame(update);
    };

    const syncAnimation = () => {
      if (canvasVisible && documentVisible && animationFrame === 0) {
        animationFrame = window.requestAnimationFrame(update);
      } else if ((!canvasVisible || !documentVisible) && animationFrame !== 0) {
        window.cancelAnimationFrame(animationFrame);
        animationFrame = 0;
        previousTimestamp = 0;
      }
    };
    const handleVisibilityChange = () => {
      documentVisible = document.visibilityState !== "hidden";
      syncAnimation();
    };
    const observer =
      "IntersectionObserver" in window
        ? new IntersectionObserver(
            (entries) => {
              canvasVisible = entries.some(
                (entry) => entry.target === canvas && entry.isIntersecting
              );
              syncAnimation();
            },
            { threshold: 0.01 }
          )
        : null;
    observer?.observe(canvas);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    syncAnimation();

    return () => {
      if (animationFrame !== 0) window.cancelAnimationFrame(animationFrame);
      observer?.disconnect();
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      layout.nodes.forEach((node, index) => {
        nodeElements[index]?.setAttribute("transform", `translate(${node.x} ${node.y})`);
      });
    };
  }, [layout]);

  if (layout.nodes.length === 0) {
    return <div className="knowledge-graph-empty">Index the workspace to build the graph.</div>;
  }

  return (
    <div className="knowledge-graph">
      <div className="knowledge-graph-meta">
        <span>{graph.totalNodes} nodes</span>
        <span>{graph.totalEdges} links</span>
        <span>{layout.nodes.filter((node) => node.focused).length} recalled</span>
      </div>
      <div className="knowledge-graph-canvas" ref={canvasRef}>
        <svg
          role="img"
          aria-label="Workspace knowledge graph"
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          preserveAspectRatio="xMidYMid meet"
          onPointerLeave={() => setHoveredId(null)}
        >
          <g className="knowledge-graph-scene" ref={sceneRef}>
            <g className="knowledge-graph-edges">
              {layout.edges.map((edge) => {
                const connected = activeId === edge.from || activeId === edge.to;
                return (
                  <line
                    key={edge.id}
                    x1={edge.source.x}
                  y1={edge.source.y}
                  x2={edge.target.x}
                  y2={edge.target.y}
                  data-related={connected || undefined}
                  data-muted={Boolean(activeId) && !connected ? true : undefined}
                  >
                    <title>{edge.kind}</title>
                  </line>
                );
              })}
            </g>
            <g className="knowledge-graph-nodes">
              {layout.nodes.map((node) => {
                const active = node.id === activeId;
                const related = activeNodeIds.has(node.id);
                const labelVisible = active || node.focused || node.degree >= 4;
                return (
                  <g
                    key={node.id}
                    className="knowledge-graph-node"
                    data-focused={node.focused || undefined}
                    data-selected={node.id === selectedId || undefined}
                    data-active={active || undefined}
                    data-related={related || undefined}
                    data-muted={Boolean(activeId) && !related ? true : undefined}
                    transform={`translate(${node.x} ${node.y})`}
                    role="button"
                    tabIndex={0}
                    aria-label={`${node.kind}: ${node.label}`}
                    onPointerEnter={() => setHoveredId(node.id)}
                    onFocus={() => setHoveredId(node.id)}
                    onBlur={() => setHoveredId(null)}
                    onClick={() => setSelectedId(node.id === selectedId ? null : node.id)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        setSelectedId(node.id === selectedId ? null : node.id);
                      }
                    }}
                  >
                    <circle
                      className="knowledge-graph-node-core"
                      r={node.radius}
                      fill={nodeColor(node.kind)}
                    />
                    <text
                      className="knowledge-graph-node-label"
                      y={node.radius + 11}
                      data-visible={labelVisible || undefined}
                    >
                      {shortNodeLabel(node.label)}
                    </text>
                    <title>{`${node.label}\n${node.sourcePath}`}</title>
                  </g>
                );
              })}
            </g>
          </g>
        </svg>
      </div>
      <div className="knowledge-graph-legend" aria-label="Graph node kinds">
        {[...new Set(layout.nodes.map((node) => node.kind))].map((kind) => (
          <span key={kind}>
            <i style={{ background: nodeColor(kind) }} />
            {kind}
          </span>
        ))}
      </div>
      {selected && (
        <div className="knowledge-graph-selection">
          <strong>{selected.label}</strong>
          <span>{selected.kind}</span>
          <small>{selected.sourcePath}</small>
        </div>
      )}
    </div>
  );
}
