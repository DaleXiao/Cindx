import { useMemo, useState } from "react";
import type { GraphNodeView, GraphStateView } from "../tauri";

type KnowledgeGraphProps = {
  graph: GraphStateView;
};

type PositionedNode = GraphNodeView & {
  x: number;
  y: number;
};

const WIDTH = 900;
const HEIGHT = 380;
const MAX_VISIBLE_NODES = 56;

const KIND_ORDER = ["file", "symbol", "tool", "decision", "claim", "task"];

function nodeColor(kind: string) {
  switch (kind) {
    case "file":
      return "#2f6f9f";
    case "symbol":
      return "#6d5d9f";
    case "tool":
      return "#2f8b68";
    case "decision":
      return "#a16824";
    case "claim":
      return "#9b4e5f";
    default:
      return "#626a73";
  }
}

function layoutNodes(nodes: GraphNodeView[]): PositionedNode[] {
  const visible = [...nodes]
    .sort((left, right) => Number(right.focused) - Number(left.focused))
    .slice(0, MAX_VISIBLE_NODES);
  const kinds = KIND_ORDER.filter((kind) => visible.some((node) => node.kind === kind));
  const unknownKinds = [...new Set(visible.map((node) => node.kind))].filter(
    (kind) => !kinds.includes(kind)
  );
  const columns = [...kinds, ...unknownKinds];
  const columnWidth = (WIDTH - 100) / Math.max(columns.length - 1, 1);

  return columns.flatMap((kind, columnIndex) => {
    const group = visible.filter((node) => node.kind === kind);
    const rowHeight = (HEIGHT - 70) / Math.max(group.length, 1);
    return group.map((node, rowIndex) => ({
      ...node,
      x: 50 + columnIndex * columnWidth,
      y: 42 + rowHeight * (rowIndex + 0.5)
    }));
  });
}

export function KnowledgeGraph({ graph }: KnowledgeGraphProps) {
  const positioned = useMemo(() => layoutNodes(graph.nodes), [graph.nodes]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = positioned.find((node) => node.id === selectedId) ?? null;
  const positions = useMemo(
    () => new Map(positioned.map((node) => [node.id, node])),
    [positioned]
  );
  const visibleEdges = graph.edges.filter(
    (edge) => positions.has(edge.from) && positions.has(edge.to)
  );

  if (positioned.length === 0) {
    return (
      <div className="knowledge-graph-empty">
        Index the workspace to build the graph.
      </div>
    );
  }

  return (
    <div className="knowledge-graph">
      <div className="knowledge-graph-meta">
        <span>{graph.totalNodes} nodes</span>
        <span>{graph.totalEdges} edges</span>
        <span>{positioned.filter((node) => node.focused).length} recalled</span>
      </div>
      <div className="knowledge-graph-canvas">
        <svg
          role="img"
          aria-label="Workspace knowledge graph"
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          preserveAspectRatio="xMidYMid meet"
        >
          <g className="knowledge-graph-edges">
            {visibleEdges.map((edge) => {
              const from = positions.get(edge.from)!;
              const to = positions.get(edge.to)!;
              const highlighted =
                selectedId === edge.from || selectedId === edge.to;
              return (
                <line
                  key={edge.id}
                  x1={from.x}
                  y1={from.y}
                  x2={to.x}
                  y2={to.y}
                  data-highlighted={highlighted || undefined}
                >
                  <title>{edge.kind}</title>
                </line>
              );
            })}
          </g>
          <g className="knowledge-graph-nodes">
            {positioned.map((node) => (
              <g
                key={node.id}
                className="knowledge-graph-node"
                data-focused={node.focused || undefined}
                data-selected={node.id === selectedId || undefined}
                transform={`translate(${node.x} ${node.y})`}
                role="button"
                tabIndex={0}
                aria-label={`${node.kind}: ${node.label}`}
                onClick={() => setSelectedId(node.id === selectedId ? null : node.id)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    setSelectedId(node.id === selectedId ? null : node.id);
                  }
                }}
              >
                {node.focused && <circle className="knowledge-graph-node-halo" r="11" />}
                <circle r={node.kind === "file" ? 6.5 : 5} fill={nodeColor(node.kind)} />
                <title>{`${node.label}\n${node.sourcePath}`}</title>
              </g>
            ))}
          </g>
        </svg>
      </div>
      <div className="knowledge-graph-legend" aria-label="Graph node kinds">
        {[...new Set(positioned.map((node) => node.kind))].map((kind) => (
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
