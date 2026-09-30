import { PracticeWizard } from "../../features/practice/practice-wizard";
import { PageShell } from "../page-shell";

export function PracticePage() {
  return (
    <>
      <PageShell id="practice" />
      <PracticeWizard />
    </>
  );
}
