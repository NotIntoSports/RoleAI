// English copy. The key set must mirror zh-CN exactly;
// this is enforced at compile time via the Dictionary type.
import type { Dictionary } from "./zh-CN";

export const en: Dictionary = {
  app: {
    startup: {
      checking: "Checking local configuration…",
      serviceStateUnavailable: "Cannot read the desktop service status",
    },
    nav: {
      primary: "Primary navigation",
      toggle: "Toggle navigation",
      localWorkspace: "Local workspace",
    },
    routes: {
      labels: {
        workspace: "Workspace",
        livestream: "Live studio",
        practice: "Mock interview",
        materials: "Materials",
        records: "Records",
        services: "Services",
        settings: "Settings",
      },
      descriptions: {
        workspace: "Keep conversations natural — you stay in control when it matters.",
        livestream: "Prepare scripts from product materials and stream a controlled virtual camera feed via OBS.",
        practice: "Practice with an AI interviewer from a question plan, then get a scored report.",
        materials: "Organize reference materials that give every answer context.",
        records: "Review conversations and keep what is worth following up on.",
        services: "Connect model and voice services to configure your AI capabilities.",
        settings: "Tune your workspace, manage roles and local data.",
      },
      capabilities: {
        workspace: [
          "Session state and controls (start/pause/resume/stop)",
          "Realtime AI replies with subtitles",
          "Manual takeover and intervention controls",
          "Audio and video connection status",
          "Meeting bridge card",
        ],
        livestream: [
          "Segmented scripts generated from local product materials",
          "Image or looping video stage",
          "Script review and segment controls",
          "OBS Virtual Camera output",
        ],
        practice: [
          "Role direction with JD and resume selection",
          "Interviewer styles (friendly/HR/strict)",
          "Question count, difficulty, and estimated duration",
          "Question plan preview, editing, and saving",
        ],
        materials: [
          "Resume import and management",
          "Knowledge base chunking and index status",
          "FTS full-text search",
          "Vector embeddings (sqlite-vec)",
          "Material preview and filters",
        ],
        records: [
          "Session history list with pagination",
          "Minutes details (summary/strengths/follow-ups/limits/evidence)",
          "Record export",
          "Record deletion (two-step confirm)",
        ],
        services: [
          "Model provider configuration and status",
          "Voice route configuration and testing",
          "Key management (Windows Credential Manager)",
          "Connection tests and health checks",
        ],
        settings: [
          "System diagnostics export",
          "Config location selection and display",
          "Meeting video output modes",
          "Assistant voice and avatar",
          "OBS and virtual camera",
          "Audio routing and device checks",
        ],
      },
    },
  },
  settings: {
    appearance: {
      language: {
        legend: "Interface language",
        system: "System",
        simplifiedChinese: "简体中文",
        english: "English",
        note: "Applies immediately. The preference is saved on this device.",
      },
    },
  },
};

export default en;
