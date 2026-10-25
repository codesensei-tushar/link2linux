import { Navigate, type RouteObject } from "react-router";

import { features } from "@/features";

import { AppLayout } from "./layout/AppLayout";

/** One route per feature at `/<id>`. Anything else goes to pairing. */
export const routes: RouteObject[] = [
  {
    path: "/",
    element: <AppLayout />,
    children: [
      { index: true, element: <Navigate to="/pairing" replace /> },
      ...features.map(({ id, Page }) => ({ path: id, element: <Page /> })),
      { path: "*", element: <Navigate to="/pairing" replace /> },
    ],
  },
];
