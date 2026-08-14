import createClient from "openapi-fetch";
import { getAuthHeaders } from "./client";
import type { paths } from "./schema";

let baseUrl = "";

export function setServerUrl(url: string) {
    baseUrl = url;
}

function getClient() {
    return createClient<paths>({
        baseUrl: baseUrl ? `${baseUrl}/api` : "/api",
        headers: getAuthHeaders(),
    });
}

// Re-export the typed client for direct use
export { getClient };
export type { paths };
