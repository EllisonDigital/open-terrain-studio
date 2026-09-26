// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 EllisonDigital
#pragma once
#include "CoreMinimal.h"
#include "Kismet/BlueprintFunctionLibrary.h"
#include "OTSLandscapeLibrary.generated.h"
class ALandscape;
class UHierarchicalInstancedStaticMeshComponent;

// UE 5.6 API target. Not compiled or exercised in Unreal in this repository.
UCLASS()
class OPENTERRAINSTUDIOIMPORT_API UOTSLandscapeLibrary : public UBlueprintFunctionLibrary
{
    GENERATED_BODY()
public:
    // Python: unreal.OTSLandscapeLibrary.create_landscape_from_png(...)
    UFUNCTION(BlueprintCallable, Category="OpenTerrainStudio")
    static ALandscape* CreateLandscapeFromPng(
        const FString& Filename, int32 Width, int32 Height,
        int32 SectionsPerComponent, int32 SectionSizeQuads,
        FVector Scale, FVector Location, const FString& Label);

    UFUNCTION(BlueprintCallable, Category="OpenTerrainStudio")
    static UHierarchicalInstancedStaticMeshComponent* CreateSpeciesInstances(ALandscape* Landscape, const FString& Species);

    UFUNCTION(BlueprintCallable, Category="OpenTerrainStudio")
    static void AddSpeciesInstances(UHierarchicalInstancedStaticMeshComponent* Component, const TArray<FTransform>& Transforms);
};
