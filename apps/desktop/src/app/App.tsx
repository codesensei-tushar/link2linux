import { createHashRouter, RouterProvider } from "react-router";

import { AppProviders } from "./providers";
import { routes } from "./routes";

// Hash routing: the app is served from static files with no server-side fallback.
const router = createHashRouter(routes);

export function App() {
  return (
    <AppProviders>
      <RouterProvider router={router} />
    </AppProviders>
  );
}
