type DisclosureTriangleProps = {
  className?: string;
};

export function DisclosureTriangle({ className }: DisclosureTriangleProps) {
  return (
    <svg
      className={`disclosure-triangle${className ? ` ${className}` : ""}`}
      viewBox="0 0 16 16"
      focusable="false"
      aria-hidden="true"
      shapeRendering="geometricPrecision"
    >
      <path d="M8 1.65c.52 0 1 .28 1.26.73l4.42 7.65c.26.45.26 1.01 0 1.46s-.74.73-1.26.73H3.58c-.52 0-1-.28-1.26-.73s-.26-1.01 0-1.46l4.42-7.65c.26-.45.74-.73 1.26-.73Z" />
    </svg>
  );
}
