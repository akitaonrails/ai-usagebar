import type { ComponentType, SVGProps } from "react";

import AnthropicMark from "~icons/aiub/anthropic";
import AnthropicApiMark from "~icons/aiub/anthropic_api";
import AntigravityMark from "~icons/aiub/antigravity";
import CopilotMark from "~icons/aiub/copilot";
import CursorMark from "~icons/aiub/cursor";
import DeepseekMark from "~icons/aiub/deepseek";
import GrokMark from "~icons/aiub/grok";
import KimiMark from "~icons/aiub/kimi";
import MinimaxMark from "~icons/aiub/minimax";
import MoonshotMark from "~icons/aiub/moonshot";
import OpenaiMark from "~icons/aiub/openai";
import OpencodeGoMark from "~icons/aiub/opencode_go";
import OpenrouterMark from "~icons/aiub/openrouter";
import ZaiMark from "~icons/aiub/zai";

import { cn } from "@/lib/utils";

type Mark = ComponentType<SVGProps<SVGSVGElement>>;

const MARKS: Record<string, Mark> = {
  anthropic: AnthropicMark,
  anthropic_api: AnthropicApiMark,
  antigravity: AntigravityMark,
  copilot: CopilotMark,
  cursor: CursorMark,
  deepseek: DeepseekMark,
  grok: GrokMark,
  kimi: KimiMark,
  minimax: MinimaxMark,
  moonshot: MoonshotMark,
  openai: OpenaiMark,
  opencode_go: OpencodeGoMark,
  openrouter: OpenrouterMark,
  zai: ZaiMark,
};

interface ProviderIconProps {
  className?: string;
  /** Pixel size, or a CSS length such as `var(--sz-icon)` so density can drive it. */
  size?: number | string;
  slug: string;
  title: string;
}

/** ProviderIcon: the provider's mark filled with the gray icon tint, or two-letter initials. */
export function ProviderIcon({ className, size = 16, slug, title }: ProviderIconProps) {
  const Mark = MARKS[slug];
  const length = typeof size === "number" ? `${size}px` : size;

  if (Mark) {
    return <Mark aria-hidden className={cn("shrink-0", className)} style={{ height: length, width: length }} />;
  }

  const initials = title.trim().slice(0, 2).toUpperCase() || "?";

  return (
    <span
      aria-hidden
      className={cn(
        "grid shrink-0 place-items-center rounded-[4px] bg-[var(--fill-quaternary)] font-bold text-[var(--label-2)]",
        className,
      )}
      style={{ fontSize: `calc(${length} * 0.5)`, height: length, width: length }}
    >
      {initials}
    </span>
  );
}
