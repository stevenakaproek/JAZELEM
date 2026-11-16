import { Hammer } from "lucide-react";
import { EmptyState, Page } from "../components/ui";

/** A tab that has not been rebuilt yet (it still works in the classic PC Tuner). */
export function ComingSoon({ title }: { title: string }) {
  return (
    <Page title={title}>
      <EmptyState icon={<Hammer />} title="Not rebuilt yet" text="This tab is still being moved to the new interface. It works as before in the classic PC Tuner." />
    </Page>
  );
}
