import { assert } from "@std/assert";

// Source-read pins for the C1 breadcrumb mounts (unified breadcrumbs plan).
// A typecheck cannot see that a page still mounts its trail, which segment is
// current, or that a href still comes from the path-builder; these pins guard
// exactly that, same rationale as chrome_pins_test and the retired shim
// source-read tests. Scoped read grants accordingly.

const TAGMATA_PAGE = new URL("./TagmataPage.svelte", import.meta.url);
const TAGMA_PROFILE_PAGE = new URL(
  "./TagmaProfilePage.svelte",
  import.meta.url,
);
const TAGMA_CHAT_PAGE = new URL("./TagmaChatPage.svelte", import.meta.url);
const ONLINE_MANAGE_PAGE = new URL(
  "./manage/OnlineManagePage.svelte",
  import.meta.url,
);
const ROOMS_PAGE = new URL("./RoomsPage.svelte", import.meta.url);
const ROOM_CONVERSATION_PAGE = new URL(
  "./RoomConversationPage.svelte",
  import.meta.url,
);
const ROOM_SETTINGS_PAGE = new URL(
  "./RoomSettingsPage.svelte",
  import.meta.url,
);
const CHANNEL_CHAT_PAGE = new URL("./ChannelChatPage.svelte", import.meta.url);
const SETTINGS_PAGE = new URL("./SettingsPage.svelte", import.meta.url);
const ACCOUNT_HUB_PAGE = new URL(
  "./account/AccountHubPage.svelte",
  import.meta.url,
);
const USER_PROFILE_PAGE = new URL("./UserProfilePage.svelte", import.meta.url);

function source(url: URL): string {
  return new TextDecoder().decode(Deno.readFileSync(url));
}

Deno.test(
  "TagmataPage mounts a single current-segment trail (root page, R1a exception)",
  { permissions: { read: [TAGMATA_PAGE] } },
  () => {
    const src = source(TAGMATA_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("label: nav_tagmata(), current: true }"),
      "the tagmata segment itself must be the current tail (no href)",
    );
  },
);

Deno.test(
  "TagmaProfilePage mounts the Tagmata-root trail with a current name tail",
  { permissions: { read: [TAGMA_PROFILE_PAGE] } },
  () => {
    const src = source(TAGMA_PROFILE_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: displayName, current: true"),
      "the profile name must be the current tail",
    );
  },
);

Deno.test(
  "TagmaChatPage keeps its parent segment on the path-builder and a current chat tail",
  { permissions: { read: [TAGMA_CHAT_PAGE] } },
  () => {
    const src = source(TAGMA_CHAT_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("tagmaDetailsPath(tagmaId)"),
      "the tagma segment href must come from the path-builder",
    );
    assert(
      src.includes("label: nav_chat(), current: true"),
      "the chat segment must be the current tail",
    );
  },
);

Deno.test(
  "OnlineManagePage keeps its parent segment on the path-builder and a current section tail",
  { permissions: { read: [ONLINE_MANAGE_PAGE] } },
  () => {
    const src = source(ONLINE_MANAGE_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("tagmaChatPath(tagmaId)"),
      "the tagma segment href must come from the path-builder",
    );
    assert(
      src.includes("sectionLabels[page](), current: true"),
      "the section label must be the current tail for every details section",
    );
  },
);

Deno.test(
  "RoomsPage mounts the Tagmata-root trail with a current rooms tail",
  { permissions: { read: [ROOMS_PAGE] } },
  () => {
    const src = source(ROOMS_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: nav_rooms(), current: true"),
      "the rooms segment must be the current tail",
    );
  },
);

Deno.test(
  "RoomConversationPage trails rooms and the room name through the PageHeader seam",
  { permissions: { read: [ROOM_CONVERSATION_PAGE] } },
  () => {
    const src = source(ROOM_CONVERSATION_PAGE);
    assert(
      src.includes("{#snippet breadcrumbs()}"),
      "the trail must ride the PageHeader breadcrumbs snippet",
    );
    assert(
      src.includes('href: "/rooms"'),
      "the rooms segment must link the rooms top level",
    );
    assert(
      src.includes("label: roomLabel, current: true"),
      "the room name must be the current tail",
    );
  },
);

Deno.test(
  "RoomSettingsPage keeps the room name as a linked middle segment",
  { permissions: { read: [ROOM_SETTINGS_PAGE] } },
  () => {
    const src = source(ROOM_SETTINGS_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("href: `/rooms/${roomId}`"),
      "the room name segment must link back to the conversation",
    );
    assert(
      src.includes("label: settings_heading(), current: true"),
      "the settings label must be the current tail",
    );
  },
);

Deno.test(
  "ChannelChatPage renders its trail only for the withTrail route shells",
  { permissions: { read: [CHANNEL_CHAT_PAGE] } },
  () => {
    const src = source(CHANNEL_CHAT_PAGE);
    assert(
      src.includes("withTrail = false"),
      "embedded hosts must keep the default of no trail",
    );
    assert(src.includes("{#if withTrail}"), "the mount must be gated");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: convLabel, current: true"),
      "the conversation label must be the current tail",
    );
  },
);

Deno.test(
  "SettingsPage mounts the Tagmata-root trail with a current settings tail",
  { permissions: { read: [SETTINGS_PAGE] } },
  () => {
    const src = source(SETTINGS_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: settings_heading(), current: true"),
      "the settings segment must be the current tail",
    );
  },
);

Deno.test(
  "AccountHubPage mounts the Tagmata-root trail with a current account tail",
  { permissions: { read: [ACCOUNT_HUB_PAGE] } },
  () => {
    const src = source(ACCOUNT_HUB_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: account_menu(), current: true"),
      "the account segment must be the current tail",
    );
  },
);

Deno.test(
  "UserProfilePage mounts the Tagmata-root trail with a current handle tail",
  { permissions: { read: [USER_PROFILE_PAGE] } },
  () => {
    const src = source(USER_PROFILE_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: handle, current: true"),
      "the profile handle must be the current tail",
    );
  },
);
