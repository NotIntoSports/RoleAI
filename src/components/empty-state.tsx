import type { ReactNode } from "react";

interface EmptyStateProps {
  title: string;
  hint?: string;
  /** 富空态（library 列表）的图标；传入后渲染 div 版式。 */
  icon?: ReactNode;
  /** 富空态的附加样式（如 library-empty）。 */
  className?: string;
}

export function EmptyState({ title, hint, icon, className }: EmptyStateProps) {
  const extra = className ? ` ${className}` : "";
  if (!icon && !hint) {
    return <p className={`empty-state${extra}`}>{title}</p>;
  }
  return (
    <div className={`empty-state${extra}`}>
      {icon}
      <p>{title}</p>
      {hint && <span className="muted">{hint}</span>}
    </div>
  );
}
