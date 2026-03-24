// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "pch.h"

#include "TerminalProtocolComServer.h"
#include "ProtocolRequestHandler.h"

#include <json/json.h>

using namespace Microsoft::WRL;

// Static state — set once before registration, never mutated.
ProtocolRequestHandler* TerminalProtocolComServer::s_handler = nullptr;

// COM class factory registration token.
static DWORD g_comRegistration = 0;
static std::shared_mutex g_mtx;

void TerminalProtocolComServer::s_setHandler(ProtocolRequestHandler* handler) noexcept
{
    s_handler = handler;
}

HRESULT TerminalProtocolComServer::s_StartListening()
try
{
    std::unique_lock lock{ g_mtx };

    const auto classFactory = Make<SimpleClassFactory<TerminalProtocolComServer>>();
    RETURN_LAST_ERROR_IF_NULL(classFactory);

    ComPtr<IUnknown> unk;
    RETURN_IF_FAILED(classFactory.As(&unk));

    RETURN_IF_FAILED(CoRegisterClassObject(
        __uuidof(TerminalProtocolComServer),
        unk.Get(),
        CLSCTX_LOCAL_SERVER,
        REGCLS_MULTIPLEUSE,
        &g_comRegistration));

    return S_OK;
}
CATCH_RETURN()

HRESULT TerminalProtocolComServer::s_StopListening()
{
    std::unique_lock lock{ g_mtx };

    if (g_comRegistration)
    {
        RETURN_IF_FAILED(CoRevokeClassObject(g_comRegistration));
        g_comRegistration = 0;
    }

    return S_OK;
}

STDMETHODIMP TerminalProtocolComServer::HandleRequest(BSTR requestJson, BSTR* responseJson)
try
{
    RETURN_HR_IF_NULL(E_POINTER, responseJson);
    *responseJson = nullptr;

    RETURN_HR_IF_NULL(E_NOT_VALID_STATE, s_handler);
    RETURN_HR_IF_NULL(E_INVALIDARG, requestJson);

    // Parse the incoming JSON request (BSTR is UTF-16 → convert to UTF-8).
    const auto reqWide = std::wstring_view(requestJson, SysStringLen(requestJson));
    const auto reqUtf8 = winrt::to_string(reqWide);

    Json::Value request;
    Json::CharReaderBuilder readerBuilder;
    std::string parseErrors;
    std::istringstream stream(reqUtf8);
    if (!Json::parseFromStream(readerBuilder, stream, &request, &parseErrors))
    {
        // Build a parse error response.
        Json::Value errResp;
        errResp["type"] = "response";
        errResp["id"] = "";
        errResp["result"] = Json::nullValue;
        Json::Value err;
        err["code"] = "parse_error";
        err["message"] = "Failed to parse request JSON: " + parseErrors;
        errResp["error"] = err;

        Json::StreamWriterBuilder writerBuilder;
        writerBuilder["indentation"] = "";
        const auto respStr = Json::writeString(writerBuilder, errResp);

        *responseJson = SysAllocString(winrt::to_hstring(respStr).c_str());
        return S_OK; // COM call succeeded; protocol error is in the response.
    }

    // Delegate to the existing request handler.
    // This is the same handler used by the pipe server — it handles all
    // method dispatch, authentication, confirmation, etc.
    const auto response = s_handler->HandleRequest(request, _authenticated);

    // Serialize the response back to JSON.
    Json::StreamWriterBuilder writerBuilder;
    writerBuilder["indentation"] = "";
    const auto respStr = Json::writeString(writerBuilder, response);

    *responseJson = SysAllocString(winrt::to_hstring(respStr).c_str());
    return S_OK;
}
CATCH_RETURN()
