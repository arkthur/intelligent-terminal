// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#pragma once

#include "ITerminalProtocolServer_h.h"

// Per-brand CLSIDs — same pattern as CTerminalHandoff.
#if defined(WT_BRANDING_RELEASE)
#define __CLSID_TerminalProtocolServer "A2E4F6B8-1C3D-4E5F-A6B7-C8D9E0F1A2B3"
#elif defined(WT_BRANDING_PREVIEW)
#define __CLSID_TerminalProtocolServer "B3F5A7C9-2D4E-4F6A-B7C8-D9E0F1A2B3C4"
#elif defined(WT_BRANDING_CANARY)
#define __CLSID_TerminalProtocolServer "C4A6B8D0-3E5F-4A7B-C8D9-E0F1A2B3C4D5"
#else
#define __CLSID_TerminalProtocolServer "D5B7C9E1-4F6A-4B8C-D9E0-F1A2B3C4D5E6"
#endif

class ProtocolRequestHandler;

struct __declspec(uuid(__CLSID_TerminalProtocolServer))
TerminalProtocolComServer : public Microsoft::WRL::RuntimeClass<Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::RuntimeClassType::ClassicCom>, ITerminalProtocolServer>
{
    // ITerminalProtocolServer
    STDMETHODIMP HandleRequest(BSTR requestJson, BSTR* responseJson) override;

    // Static setup — must be called before s_StartListening().
    static void s_setHandler(ProtocolRequestHandler* handler) noexcept;

    // Register/revoke the COM class factory with the SCM.
    static HRESULT s_StartListening();
    static HRESULT s_StopListening();

    // Register the automation proxy/stub for our interface IID.
    // Must be called in BOTH server and client processes before COM calls.
    static HRESULT s_RegisterAutomationProxy();

private:
    // Per-instance authentication state (mirrors pipe connection state).
    bool _authenticated = false;

    // Shared across all instances — set once at startup, never changes.
    static ProtocolRequestHandler* s_handler;
};

// Disable warnings from the CoCreatableClass macro.
#pragma warning(push)
#pragma warning(disable : 26477) // Macro uses 0/NULL over nullptr.
#pragma warning(disable : 26476) // Macro uses naked union over variant.
CoCreatableClass(TerminalProtocolComServer);
#pragma warning(pop)
