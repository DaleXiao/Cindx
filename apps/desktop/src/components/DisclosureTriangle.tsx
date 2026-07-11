import { Triangle } from "lucide-react";

type DisclosureTriangleProps = {
  className?: string;
};

export function DisclosureTriangle({ className }: DisclosureTriangleProps) {
  return (
    <Triangle
      className={`disclosure-triangle${className ? ` ${className}` : ""}`}
      aria-hidden="true"
    />
  );
}
