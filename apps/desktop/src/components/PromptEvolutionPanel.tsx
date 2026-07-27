import { Dna } from "lucide-react";
import type { Phase4State, ProviderConfigInput } from "../tauri";

type PromptEvolutionPanelProps = {
  phase4: Phase4State | null;
  providerBusy: boolean;
  providerDraft: ProviderConfigInput | null;
  onToggle: (enabled: boolean) => Promise<void>;
};

function formatTokenCount(tokens: number) {
  if (tokens < 1000) return String(tokens);
  if (tokens < 1_000_000) return `${(tokens / 1000).toFixed(tokens < 10_000 ? 1 : 0)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}

function formatObservedDuration(durationMs: number) {
  if (durationMs <= 0) return "No data";
  if (durationMs < 60_000) return `${Math.max(1, Math.round(durationMs / 1000))}s`;
  return `${Math.round(durationMs / 60_000)}m`;
}

function profileLabel(effort: string) {
  if (effort === "fast") return "Fast";
  if (effort === "pro") return "Pro";
  return "Auto";
}

function effortStatus(effort: Phase4State["promptEvolution"]["efforts"][number]) {
  if (!effort.applicable) return "Not applicable";
  if (effort.evaluationInflight) return "Evaluating";
  if (effort.rolloutStatus === "canary") return `Canary ${effort.canaryPercent}%`;
  if (effort.rolloutStatus === "evaluating") return "Gathering evidence";
  if (effort.rolloutStatus === "rolled_back") return "Rolled back";
  if (effort.rolloutStatus === "promoted") return "Promoted";
  if (effort.status === "disabled") return "Off";
  return "Stable";
}

function readinessLabel(effort: Phase4State["promptEvolution"]["efforts"][number]) {
  switch (effort.readiness) {
    case "not_applicable":
      return "Single-model path";
    case "disabled":
      return "Enable evolution";
    case "evaluating":
      return "Running offline pair";
    case "collecting_dataset": {
      const remaining = Math.max(0, 3 - effort.datasetCases);
      return `Need ${remaining} completed task${remaining === 1 ? "" : "s"}`;
    }
    case "collecting_train_evidence":
      return "Collect train evidence";
    case "collecting_holdout_evidence":
      return "Collect holdout evidence";
    case "selecting_frontier":
      return "Select Pareto frontier";
    case "canary":
      return "Measure canary";
    case "rolled_back":
      return "Explore after rollback";
    case "promoted":
      return "Monitor promoted profile";
    default:
      return effort.nextMode.replace(/_/g, " ");
  }
}

function profileStatus(profile: Phase4State["promptEvolution"]["profiles"][number]) {
  if (profile.champion) return "Champion";
  if (profile.next) return "Next";
  if (profile.frontier) return "Frontier";
  if (profile.runs) return "Observed";
  return "Queued";
}

export function PromptEvolutionPanel({
  phase4,
  providerBusy,
  providerDraft,
  onToggle
}: PromptEvolutionPanelProps) {
  const evolution = phase4?.promptEvolution;
  return (
    <section className="settings-section" data-settings-group="models">
      <div className="prompt-evolution-title">
        <div className="section-title">
          <Dna size={17} aria-hidden="true" />
          <h2>Genetic Pareto</h2>
          {evolution?.evaluationInflight && (
            <span className="prompt-evolution-running">Evaluating in background</span>
          )}
        </div>
        {providerDraft && (
          <label className="settings-switch">
            <input
              type="checkbox"
              checked={providerDraft.promptEvolutionEnabled}
              disabled={providerBusy}
              onChange={(event) => void onToggle(event.target.checked)}
            />
            <span className="settings-switch-track" aria-hidden="true">
              <span />
            </span>
            <span>{providerDraft.promptEvolutionEnabled ? "On" : "Off"}</span>
          </label>
        )}
      </div>
      <p className="settings-section-copy">
        Candidate harnesses execute in an isolated arena before promotion. Same-task paired runs
        train the population, historical replay runs provide holdout evidence, and a direct
        stable-versus-challenger Wilson gate controls staged canary rollout with automatic
        rollback.
      </p>
      <div className="prompt-evolution-summary" aria-label="Evolution overview">
        <span><strong>{evolution?.observedRuns ?? 0}</strong> observed</span>
        <span><strong>{evolution?.pairedRuns ?? 0}</strong> paired</span>
        <span><strong>{evolution?.replayRuns ?? 0}</strong> replay</span>
        <span><strong>{evolution?.reflectionPackets ?? 0}</strong> reflections</span>
        <span><strong>{evolution?.learnedProfiles ?? 0}</strong> learned</span>
        <span><strong>{evolution?.populationSize ?? 0}</strong> profiles</span>
        <span><strong>{evolution?.generation ?? 0}</strong> generation</span>
        <span><strong>{evolution?.frontierProfiles ?? 0}</strong> frontier</span>
      </div>
      <div className="prompt-evolution-table-wrap">
        <table className="prompt-evolution-table">
          <caption>Rollout by effort</caption>
          <thead>
            <tr>
              <th scope="col">Effort</th>
              <th scope="col">State</th>
              <th scope="col">Evidence</th>
              <th scope="col">Score</th>
              <th scope="col">Confidence</th>
              <th scope="col">Progress</th>
              <th scope="col">Rollbacks</th>
              <th scope="col">Next</th>
            </tr>
          </thead>
          <tbody>
            {(evolution?.efforts ?? []).map((effort) => (
              <tr key={effort.effort} title={effort.championId ?? undefined}>
                <th scope="row">{profileLabel(effort.effort)}</th>
                <td>
                  <span
                    className={`prompt-evolution-status ${effort.evaluationInflight ? "evaluating" : effort.rolloutStatus}`}
                  >
                    {effortStatus(effort)}
                  </span>
                </td>
                <td title={`${effort.datasetCases} offline cases (${effort.datasetTrainCases} train · ${effort.datasetHoldoutCases} holdout) · ${effort.reflectionPackets} feedback reflections · ${effort.learnedProfiles} learned profiles`}>
                  {effort.applicable
                    ? `${effort.pairedRuns}/${effort.requiredPairedRuns} · ${effort.replayRuns}/${effort.requiredReplayRuns} · R${effort.reflectionPackets}`
                    : "-"}
                </td>
                <td>{effort.championScore === null ? "-" : `${Math.round(effort.championScore * 100)}%`}</td>
                <td>{effort.promotionConfidence === null ? "-" : `${Math.round(effort.promotionConfidence * 100)}%`}</td>
                <td title={`Ready ${effort.readyProfiles} · Stagnant ${effort.stagnantGenerations}/3`}>
                  Gen {effort.evaluatedGenerations} · Ready {effort.readyProfiles}
                </td>
                <td>{effort.rollbackCount}</td>
                <td title={effort.freezeReason ?? undefined}>{readinessLabel(effort)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="prompt-evolution-table-wrap">
        <table className="prompt-evolution-table prompt-evolution-profile-table">
          <caption>Candidate profiles</caption>
          <thead>
            <tr>
              <th scope="col">Profile</th>
              <th scope="col">Evidence</th>
              <th scope="col">Success</th>
              <th scope="col">Quality</th>
              <th scope="col">Reward</th>
              <th scope="col">Signal</th>
              <th scope="col">Efficiency</th>
              <th scope="col">State</th>
            </tr>
          </thead>
          <tbody>
            {(evolution?.profiles ?? [])
              .filter((profile) => profile.next || profile.frontier || profile.runs > 0)
              .map((profile) => (
                <tr key={profile.id} title={profile.id}>
                  <th className="prompt-evolution-profile-cell" scope="row">
                    <strong>{profileLabel(profile.effort)}</strong>
                    <small>{profile.learned ? "Learned" : "Genetic"} · Gen {profile.generation}</small>
                  </th>
                  <td title={`${profile.reflectionRuns} feedback reflections`}>
                    {profile.trainRuns} / {profile.holdoutRuns} · R{profile.reflectionRuns}
                  </td>
                  <td>{profile.runs ? `${Math.round(profile.successRate * 100)}%` : "-"}</td>
                  <td>{profile.averageQuality === null ? "-" : `${Math.round(profile.averageQuality * 100)}%`}</td>
                  <td>{profile.averageReward === null ? "-" : `${Math.round(profile.averageReward * 100)}%`}</td>
                  <td className="prompt-evolution-signal-cell">
                    <span>
                      {profile.averageRelativeReward === null
                        ? "-"
                        : `${profile.averageRelativeReward >= 0 ? "+" : ""}${Math.round(profile.averageRelativeReward * 100)}%`}
                    </span>
                    <small>
                      Credit {profile.averageStepCredit === null ? "-" : `${Math.round(profile.averageStepCredit * 100)}%`}
                    </small>
                  </td>
                  <td className="prompt-evolution-efficiency-cell">
                    <span>{formatObservedDuration(profile.averageLatencyMs)}</span>
                    <small>{profile.averageTokens ? formatTokenCount(profile.averageTokens) : "-"}</small>
                  </td>
                  <td>
                    <span className={profile.frontier || profile.next ? "pareto-frontier active" : "pareto-frontier"}>
                      {profileStatus(profile)}
                    </span>
                  </td>
                </tr>
              ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
