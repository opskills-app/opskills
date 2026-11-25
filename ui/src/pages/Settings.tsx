import { Card, CardHeader, PageHeader, Avatar, Button, SampleBanner } from "../components/ui";

const members = [
  { name: "Maya Chen", role: "Admin" },
  { name: "Jonas Weber", role: "Operator" },
  { name: "Priya Natarajan", role: "Operator" },
  { name: "Leo Martins", role: "Viewer" },
];

export function SettingsPage() {
  return (
    <>
      <PageHeader title="Settings" subtitle="Workspace members, access and integrations." />
      <SampleBanner>Members and roles arrive in a later release. This version has one built-in admin user.</SampleBanner>
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Members" action={<Button size="sm">Invite</Button>} />
          <ul className="divide-y divide-line">
            {members.map((m) => (
              <li key={m.name} className="flex items-center gap-3 px-6 py-3 sm:px-8">
                <Avatar name={m.name} size="size-7" />
                <span className="flex-1 text-sm font-medium">{m.name}</span>
                <select defaultValue={m.role} className="h-7 rounded-md border border-line bg-surface px-2 text-xs">
                  <option>Admin</option>
                  <option>Operator</option>
                  <option>Viewer</option>
                </select>
              </li>
            ))}
          </ul>
        </Card>
        <Card>
          <CardHeader title="Roles" hint="What each role can do in this workspace" />
          <table className="w-full text-sm">
            <thead>
              <tr className="border-y border-line bg-surface-2 text-left text-xs text-muted">
                <th className="px-6 py-3 font-medium sm:px-8">Permission</th>
                <th className="px-2 py-2 text-center font-medium">Admin</th>
                <th className="px-2 py-2 text-center font-medium">Operator</th>
                <th className="px-2 py-2 text-center font-medium">Viewer</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {[
                ["View runbooks and runs", 1, 1, 1],
                ["Start and cancel runs", 1, 1, 0],
                ["Approve gated steps", 1, 1, 0],
                ["Edit runbooks", 1, 0, 0],
                ["Manage vault and members", 1, 0, 0],
              ].map(([label, ...cells]) => (
                <tr key={label as string}>
                  <td className="px-6 py-3 sm:px-8">{label}</td>
                  {cells.map((c, i) => (
                    <td key={i} className="px-2 py-2 text-center">
                      {c ? <span className="text-accent">●</span> : <span className="text-line-strong">○</span>}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      </div>
    </>
  );
}
