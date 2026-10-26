import { createHashRouter, RouterProvider } from "react-router";

import { AppProviders } from "./providers";
import { routes } from "./routes";
import { StartupGate } from "./startup/StartupGate";

// Hash routing: the app is served from static files with no server-side fallback.
const router = createHashRouter(routes);

export function App() {
  return (
    <AppProviders>
      <StartupGate>
        <RouterProvider router={router} />
      </StartupGate>
    </AppProviders>
  );
}
