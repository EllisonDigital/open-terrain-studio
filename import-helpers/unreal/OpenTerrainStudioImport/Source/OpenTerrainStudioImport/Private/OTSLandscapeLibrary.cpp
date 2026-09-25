// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 EllisonDigital
#include "OTSLandscapeLibrary.h"
#include "Editor.h"
#include "Engine/World.h"
#include "Landscape.h"
#include "LandscapeEditorModule.h"
#include "LandscapeFileFormatInterface.h"
#include "Misc/Paths.h"
#include "Modules/ModuleManager.h"
#include "ScopedTransaction.h"

IMPLEMENT_MODULE(FDefaultModuleImpl, OpenTerrainStudioImport)

ALandscape* UOTSLandscapeLibrary::CreateLandscapeFromPng(
    const FString& Filename, int32 Width, int32 Height,
    int32 SectionsPerComponent, int32 SectionSizeQuads,
    FVector Scale, FVector Location, const FString& Label)
{
    const auto Fail = [](const FString& Message) -> ALandscape*
    {
        UE_LOG(LogTemp, Error, TEXT("OpenTerrainStudio: %s"), *Message);
        return nullptr;
    };
    UWorld* World = GEditor ? GEditor->GetEditorWorldContext().World() : nullptr;
    if (!World || World->WorldType != EWorldType::Editor || World->GetWorldPartition())
    {
        return Fail(TEXT("Use a non-World-Partition editor level (not PIE). Tiled/partitioned import is not implemented."));
    }
    if (Width < 2 || Height < 2 || Width > 16384 || Height > 16384 ||
        (SectionsPerComponent != 1 && SectionsPerComponent != 2) ||
        (SectionSizeQuads != 7 && SectionSizeQuads != 15 && SectionSizeQuads != 31 &&
         SectionSizeQuads != 63 && SectionSizeQuads != 127 && SectionSizeQuads != 255))
    {
        return Fail(TEXT("Invalid Landscape section dimensions."));
    }
    const int32 ComponentQuads = SectionsPerComponent * SectionSizeQuads;
    if ((Width - 1) % ComponentQuads || (Height - 1) % ComponentQuads ||
        (Width - 1) / ComponentQuads > 32 || (Height - 1) / ComponentQuads > 32)
    {
        return Fail(TEXT("Heightmap must exactly fit the component grid (at most 32 per axis)."));
    }
    if (!FMath::IsFinite(Scale.X) || !FMath::IsFinite(Scale.Y) || !FMath::IsFinite(Scale.Z) ||
        !FMath::IsFinite(Location.X) || !FMath::IsFinite(Location.Y) || !FMath::IsFinite(Location.Z) ||
        Scale.X <= 0 || Scale.Y <= 0 || Scale.Z <= 0 ||
        !FPaths::GetExtension(Filename).Equals(TEXT("png"), ESearchCase::IgnoreCase))
    {
        return Fail(TEXT("Expected a PNG filename and finite positive scale."));
    }
    ILandscapeEditorModule& Module = FModuleManager::LoadModuleChecked<ILandscapeEditorModule>("LandscapeEditor");
    const ILandscapeHeightmapFileFormat* Format = Module.GetHeightmapFormatByExtension(TEXT(".png"));
    if (!Format)
    {
        return Fail(TEXT("The editor has no PNG Landscape importer."));
    }
    const FLandscapeFileResolution Resolution{static_cast<uint32>(Width), static_cast<uint32>(Height)};
    const FLandscapeHeightmapImportData Data = Format->Import(*Filename, Resolution);
    if (Data.ResultCode == ELandscapeImportResult::Error || Data.Data.Num() != Width * Height)
    {
        return Fail(FString(TEXT("Heightmap import failed: ")) + Data.ErrorMessage.ToString());
    }
    // No conversion through Texture2D/GPU: the native importer supplies uint16s.
    TMap<FGuid, TArray<uint16>> Heights;
    Heights.Add(FGuid(), Data.Data);
    TMap<FGuid, TArray<FLandscapeImportLayerInfo>> Layers;
    Layers.Add(FGuid(), TArray<FLandscapeImportLayerInfo>());
    const FScopedTransaction Transaction(NSLOCTEXT("OpenTerrainStudio", "Import", "Import OpenTerrainStudio Landscape"));
    World->Modify();
    FActorSpawnParameters Spawn;
    Spawn.ObjectFlags |= RF_Transactional;
    ALandscape* Landscape = World->SpawnActor<ALandscape>(Location, FRotator::ZeroRotator, Spawn);
    if (!Landscape)
    {
        return Fail(TEXT("Could not spawn Landscape."));
    }
    Landscape->SetActorLabel(Label);
    Landscape->SetActorScale3D(Scale);
    Landscape->Import(FGuid::NewGuid(), 0, 0, Width - 1, Height - 1,
        SectionsPerComponent, SectionSizeQuads, Heights, *Filename, Layers,
        ELandscapeImportAlphamapType::Additive, TArrayView<const FLandscapeLayer>());
    if (Landscape->LandscapeComponents.IsEmpty())
    {
        World->DestroyActor(Landscape);
        return Fail(TEXT("Native Landscape import created no components."));
    }
    Landscape->CreateLandscapeInfo();
    Landscape->RegisterAllComponents();
    Landscape->PostEditChange();
    Landscape->MarkPackageDirty();
    return Landscape;
}
