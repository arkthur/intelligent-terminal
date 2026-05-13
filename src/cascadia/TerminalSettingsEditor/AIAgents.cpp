// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "pch.h"
#include "AIAgents.h"
#include "AIAgents.g.cpp"

using namespace winrt::Windows::UI::Xaml::Navigation;
using namespace winrt::Microsoft::Terminal::Settings::Model;

namespace winrt::Microsoft::Terminal::Settings::Editor::implementation
{
    AIAgents::AIAgents()
    {
        InitializeComponent();
    }

    void AIAgents::OnNavigatedTo(const NavigationEventArgs& e)
    {
        const auto args = e.Parameter().as<Editor::NavigateToPageArgs>();
        _ViewModel = args.ViewModel().as<Editor::AIAgentsViewModel>();
        BringIntoViewWhenLoaded(args.ElementToFocus());
    }
}
