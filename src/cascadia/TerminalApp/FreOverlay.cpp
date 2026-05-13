// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "pch.h"
#include "FreOverlay.h"
#include "FreOverlay.g.cpp"

#include <LibraryResources.h>

using namespace winrt::Windows::Foundation;
using namespace winrt::Windows::UI::Xaml;
using namespace winrt::Windows::UI::Xaml::Input;
using namespace winrt::Windows::UI::Xaml::Media;
using namespace winrt::Windows::UI::Xaml::Controls;

namespace winrt::TerminalApp::implementation
{
    FreOverlay::FreOverlay()
    {
        InitializeComponent();

        // When the overlay (or its parent window) resizes, clamp the dialog
        // so it doesn't end up off-screen.
        _rootSizeChangedToken = RootGrid().SizeChanged(
            [weakThis = get_weak()](const auto& /*sender*/, const SizeChangedEventArgs& /*args*/) {
                if (auto self = weakThis.get())
                {
                    self->_ClampDialogPosition();
                }
            });

        // Pre-load per-section detail strings.
        _detailTitles = {
            RS_(L"FreOverlay_DetailTitle"),
            RS_(L"FreOverlay_Detail2Title"),
            RS_(L"FreOverlay_Detail3Title"),
            RS_(L"FreOverlay_Detail4Title"),
        };
        _detailDescs = {
            RS_(L"FreOverlay_DetailDescription"),
            RS_(L"FreOverlay_Detail2Description"),
            RS_(L"FreOverlay_Detail3Description"),
            RS_(L"FreOverlay_Detail4Description"),
        };
    }

    // ── Localized string getters (x:Bind, evaluated once) ───────────────

    winrt::hstring FreOverlay::FreTitle()       { return RS_(L"FreOverlay_Title"); }
    winrt::hstring FreOverlay::Card1Title()     { return RS_(L"FreOverlay_Card1Title"); }
    winrt::hstring FreOverlay::Card1Description() { return RS_(L"FreOverlay_Card1Description"); }
    winrt::hstring FreOverlay::Card2Title()     { return RS_(L"FreOverlay_Card2Title"); }
    winrt::hstring FreOverlay::Card2Description() { return RS_(L"FreOverlay_Card2Description"); }
    winrt::hstring FreOverlay::Card3Title()     { return RS_(L"FreOverlay_Card3Title"); }
    winrt::hstring FreOverlay::Card3Description() { return RS_(L"FreOverlay_Card3Description"); }
    winrt::hstring FreOverlay::Card4Title()     { return RS_(L"FreOverlay_Card4Title"); }
    winrt::hstring FreOverlay::Card4Description() { return RS_(L"FreOverlay_Card4Description"); }
    winrt::hstring FreOverlay::DetailTitle()    { return _detailTitles[0]; }
    winrt::hstring FreOverlay::DetailDescription() { return _detailDescs[0]; }
    winrt::hstring FreOverlay::DetailLink()     { return RS_(L"FreOverlay_DetailLink"); }
    winrt::hstring FreOverlay::NextButtonText() { return RS_(L"FreOverlay_NextButton"); }

    // ── Navigation ──────────────────────────────────────────────────────

    void FreOverlay::_OnNavItemTapped(const IInspectable& sender,
                                      const winrt::Windows::UI::Xaml::Input::TappedRoutedEventArgs& /*args*/)
    {
        if (const auto fe = sender.try_as<FrameworkElement>())
        {
            if (const auto tag = fe.Tag())
            {
                const auto idx = winrt::unbox_value<winrt::hstring>(tag);
                _SelectNavItem(std::stoi(winrt::to_string(idx)));
            }
        }
    }

    void FreOverlay::_SelectNavItem(int32_t index)
    {
        if (index < 0 || index >= NavItemCount || index == _selectedIndex)
            return;

        // Clear old selection
        const Border bgBorders[] = { NavBg0(), NavBg1(), NavBg2(), NavBg3() };
        const winrt::Windows::UI::Xaml::Shapes::Rectangle selRects[] = { NavSel0(), NavSel1(), NavSel2(), NavSel3() };
        auto transparent = SolidColorBrush{ winrt::Windows::UI::Colors::Transparent() };
        bgBorders[_selectedIndex].Background(transparent);
        bgBorders[_selectedIndex].BorderBrush(transparent);
        selRects[_selectedIndex].Visibility(Visibility::Collapsed);

        // Set new selection
        _selectedIndex = index;
        auto selectedBrush = SolidColorBrush{
            winrt::Windows::UI::ColorHelper::FromArgb(0x0A, 0xFF, 0xFF, 0xFF) };
        bgBorders[_selectedIndex].Background(selectedBrush);
        bgBorders[_selectedIndex].BorderBrush(selectedBrush);
        selRects[_selectedIndex].Visibility(Visibility::Visible);

        // Update detail text
        DetailTitleText().Text(_detailTitles[index]);
        DetailDescRun().Text(_detailDescs[index]);

        // "Learn more" link only on the first tab
        if (index == 0)
        {
            DetailLinkSpacer().Text(L" ");
            DetailLinkRun().Text(RS_(L"FreOverlay_DetailLink"));
        }
        else
        {
            DetailLinkSpacer().Text(L"");
            DetailLinkRun().Text(L"");
        }
    }

    // ── Button handlers ─────────────────────────────────────────────────

    void FreOverlay::_OnNextButtonClick(const IInspectable& /*sender*/,
                                        const RoutedEventArgs& /*args*/)
    {
        Completed.raise(*this, nullptr);
    }

    void FreOverlay::_OnCloseButtonClick(const IInspectable& /*sender*/,
                                         const RoutedEventArgs& /*args*/)
    {
        Completed.raise(*this, nullptr);
    }

    // ── Drag-to-move handlers ───────────────────────────────────────────

    void FreOverlay::_OnTitleBarPointerPressed(const IInspectable& /*sender*/,
                                               const PointerRoutedEventArgs& e)
    {
        const auto point = e.GetCurrentPoint(RootGrid());
        // Only react to the primary button (left-click / touch / pen primary).
        if (point.Properties().IsRightButtonPressed() || point.Properties().IsMiddleButtonPressed())
        {
            return;
        }

        _titleBarDragging = TitleBarDragArea().CapturePointer(e.Pointer());
        if (!_titleBarDragging)
        {
            return;
        }

        _dragStartPointer = point.Position();
        _dragStartTranslateX = DialogTranslate().X();
        _dragStartTranslateY = DialogTranslate().Y();
        e.Handled(true);
    }

    void FreOverlay::_OnTitleBarPointerMoved(const IInspectable& /*sender*/,
                                             const PointerRoutedEventArgs& e)
    {
        if (!_titleBarDragging)
        {
            return;
        }

        const auto point = e.GetCurrentPoint(RootGrid()).Position();
        const auto deltaX = point.X - _dragStartPointer.X;
        const auto deltaY = point.Y - _dragStartPointer.Y;

        DialogTranslate().X(_dragStartTranslateX + deltaX);
        DialogTranslate().Y(_dragStartTranslateY + deltaY);

        _ClampDialogPosition();

        e.Handled(true);
    }

    void FreOverlay::_OnTitleBarPointerReleased(const IInspectable& /*sender*/,
                                                const PointerRoutedEventArgs& e)
    {
        if (_titleBarDragging)
        {
            TitleBarDragArea().ReleasePointerCapture(e.Pointer());
        }
        _titleBarDragging = false;
        e.Handled(true);
    }

    void FreOverlay::_OnTitleBarPointerCaptureLost(const IInspectable& /*sender*/,
                                                   const PointerRoutedEventArgs& /*e*/)
    {
        _titleBarDragging = false;
    }

    // ── Clamping & reset ────────────────────────────────────────────────

    void FreOverlay::_ClampDialogPosition()
    {
        const auto gridW = RootGrid().ActualWidth();
        const auto gridH = RootGrid().ActualHeight();
        const auto dlgW = DialogViewbox().ActualWidth();
        const auto dlgH = DialogViewbox().ActualHeight();

        if (gridW <= 0 || gridH <= 0)
        {
            return;
        }

        // The dialog is HorizontalAlignment=Center, VerticalAlignment=Center,
        // so at TranslateTransform (0,0) it sits in the middle.  The maximum
        // offset in each direction before the dialog leaves the visible area:
        const auto maxX = (gridW - dlgW) / 2.0;
        const auto maxY = (gridH - dlgH) / 2.0;

        // If the window is smaller than the dialog, allow zero offset.
        const auto clampX = (std::max)(0.0, maxX);
        const auto clampY = (std::max)(0.0, maxY);

        auto x = DialogTranslate().X();
        auto y = DialogTranslate().Y();

        x = (std::max)(-clampX, (std::min)(x, clampX));
        y = (std::max)(-clampY, (std::min)(y, clampY));

        DialogTranslate().X(x);
        DialogTranslate().Y(y);
    }

    void FreOverlay::ResetDragOffset()
    {
        DialogTranslate().X(0);
        DialogTranslate().Y(0);
        _titleBarDragging = false;
    }
}
